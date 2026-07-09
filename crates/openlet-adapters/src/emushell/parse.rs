//! Lower a tree-sitter-bash CST into our typed [`Script`] AST.
//!
//! The CST is stringly-typed; every node kind is a `&str`. We match on
//! those kinds once here so the evaluator only ever sees the closed
//! [`Node`] / [`Command`] / [`Word`] enums. Unrecognized constructs are
//! skipped rather than aborting the parse — tree-sitter is error-tolerant
//! and a partial script should still run the parts we understand.

use tree_sitter::{Node as TsNode, Parser};

use super::ast::{AndOr, AndOrOp, Command, Frag, Node, Script, Word};
use super::error::ShellParseError;

/// Hard cap on the source length we will parse. A multi-megabyte command
/// is either a mistake or an attempt to blow parse-time memory; reject it
/// before tree-sitter allocates a CST for it.
const MAX_SOURCE_BYTES: usize = 1024 * 1024;

/// Parse a bash script string into our AST.
pub fn parse(src: &str) -> Result<Script, ShellParseError> {
    if src.len() > MAX_SOURCE_BYTES {
        return Err(ShellParseError(format!(
            "command too large: {} bytes > {MAX_SOURCE_BYTES} limit",
            src.len()
        )));
    }
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .map_err(|e| ShellParseError(format!("load bash grammar: {e}")))?;
    let tree = parser
        .parse(src, None)
        .ok_or_else(|| ShellParseError("parser returned no tree".into()))?;
    Ok(lower_script(src, tree.root_node()))
}

/// Lower a `program` / `do_group` / `then`-body region: a run of
/// statements. We accept any node whose children are statements.
fn lower_script(src: &str, node: TsNode) -> Script {
    let mut items = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(item) = lower_statement(src, child) {
            items.push(item);
        }
    }
    Script { items }
}

/// Lower a single statement into an `AndOr`. Returns `None` for
/// punctuation / comments / structural tokens we don't execute.
fn lower_statement(src: &str, node: TsNode) -> Option<AndOr> {
    match node.kind() {
        "list" => lower_list(src, node),
        "pipeline" => Some(AndOr {
            head: Node::Pipeline(lower_pipeline(src, node)),
            tail: Vec::new(),
        }),
        "command" => Some(AndOr {
            head: Node::Pipeline(vec![lower_command(src, node)]),
            tail: Vec::new(),
        }),
        "redirected_statement" => Some(AndOr {
            head: Node::Pipeline(vec![lower_redirected(src, node)]),
            tail: Vec::new(),
        }),
        "for_statement" => lower_for(src, node).map(single),
        "while_statement" => lower_while(src, node).map(single),
        "if_statement" => lower_if(src, node).map(single),
        "variable_assignment" => {
            // A bare `VAR=value` statement: model as a command with only
            // an assignment and no argv.
            let mut cmd = Command::default();
            if let Some(a) = lower_assignment(src, node) {
                cmd.assignments.push(a);
            }
            Some(AndOr {
                head: Node::Pipeline(vec![cmd]),
                tail: Vec::new(),
            })
        }
        _ => None,
    }
}

fn single(head: Node) -> AndOr {
    AndOr {
        head,
        tail: Vec::new(),
    }
}

/// `list` nodes carry `&&` / `||` chains. tree-sitter nests them, so we
/// flatten left-to-right into a head + `(op, node)` tail.
fn lower_list(src: &str, node: TsNode) -> Option<AndOr> {
    let mut parts: Vec<AndOr> = Vec::new();
    let mut ops: Vec<AndOrOp> = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "&&" => ops.push(AndOrOp::And),
            "||" => ops.push(AndOrOp::Or),
            ";" | "\n" => {}
            _ => {
                if let Some(st) = lower_statement(src, child) {
                    parts.push(st);
                }
            }
        }
    }
    let mut iter = parts.into_iter();
    let first = iter.next()?;
    let head = first.head;
    let mut tail = first.tail;
    for (op, part) in ops.into_iter().zip(iter) {
        tail.push((op, part.head));
        tail.extend(part.tail);
    }
    Some(AndOr { head, tail })
}

fn lower_pipeline(src: &str, node: TsNode) -> Vec<Command> {
    let mut cmds = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "command" => cmds.push(lower_command(src, child)),
            "redirected_statement" => cmds.push(lower_redirected(src, child)),
            _ => {}
        }
    }
    cmds
}

fn lower_redirected(src: &str, node: TsNode) -> Command {
    let mut cmd = Command::default();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "command" => cmd = lower_command(src, child),
            "file_redirect" => apply_redirect(src, child, &mut cmd),
            _ => {}
        }
    }
    cmd
}

/// A `file_redirect` node: `> f`, `>> f`, `< f`. The operator token tells
/// direction + append; the word child is the target.
fn apply_redirect(src: &str, node: TsNode, cmd: &mut Command) {
    let mut append = false;
    let mut is_out = true;
    let mut target: Option<Word> = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            ">" => {
                is_out = true;
                append = false;
            }
            ">>" => {
                is_out = true;
                append = true;
            }
            "<" => is_out = false,
            _ if is_word_kind(child.kind()) => target = Some(lower_word(src, child)),
            _ => {}
        }
    }
    if let Some(t) = target {
        if is_out {
            cmd.redirect_out = Some((t, append));
        } else {
            cmd.redirect_in = Some(t);
        }
    }
}

fn lower_command(src: &str, node: TsNode) -> Command {
    let mut cmd = Command::default();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "command_name" => {
                // command_name wraps a single word/string/expansion.
                let mut nc = child.walk();
                for inner in child.children(&mut nc) {
                    cmd.words.push(lower_word(src, inner));
                }
            }
            "variable_assignment" => {
                if let Some(a) = lower_assignment(src, child) {
                    cmd.assignments.push(a);
                }
            }
            "file_redirect" => apply_redirect(src, child, &mut cmd),
            k if is_word_kind(k) => cmd.words.push(lower_word(src, child)),
            _ => {}
        }
    }
    cmd
}

fn lower_assignment(src: &str, node: TsNode) -> Option<(String, Word)> {
    let mut name: Option<String> = None;
    let mut value = Word::default();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "variable_name" => name = Some(node_text(src, child).to_string()),
            "=" => {}
            k if is_word_kind(k) => value = lower_word(src, child),
            _ => {}
        }
    }
    name.map(|n| (n, value))
}

fn lower_for(src: &str, node: TsNode) -> Option<Node> {
    let mut var = String::new();
    let mut items = Vec::new();
    let mut body = Script::default();
    let mut seen_in = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "variable_name" if var.is_empty() => var = node_text(src, child).to_string(),
            "in" => seen_in = true,
            "do_group" => body = lower_script(src, child),
            k if is_word_kind(k) && seen_in => items.push(lower_word(src, child)),
            _ => {}
        }
    }
    if var.is_empty() {
        return None;
    }
    Some(Node::For {
        var,
        items,
        body: Box::new(body),
    })
}

fn lower_while(src: &str, node: TsNode) -> Option<Node> {
    let mut cond = Script::default();
    let mut body = Script::default();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "do_group" => body = lower_script(src, child),
            // The condition is the statement(s) between `while` and `do`.
            k if k != "while" && k != "do_group" => {
                if let Some(st) = lower_statement(src, child) {
                    cond.items.push(st);
                }
            }
            _ => {}
        }
    }
    Some(Node::While {
        cond: Box::new(cond),
        body: Box::new(body),
    })
}

fn lower_if(src: &str, node: TsNode) -> Option<Node> {
    let mut cond = Script::default();
    let mut then_body = Script::default();
    let mut else_body: Option<Script> = None;
    let mut phase = IfPhase::Cond;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "then" => phase = IfPhase::Then,
            "else_clause" => else_body = Some(lower_script(src, child)),
            "elif_clause" => {
                // Model elif as a nested if in the else branch.
                if let Some(nested) = lower_if(src, child) {
                    else_body = Some(Script {
                        items: vec![single(nested)],
                    });
                }
            }
            "fi" | "if" => {}
            _ => match phase {
                IfPhase::Cond => {
                    if let Some(st) = lower_statement(src, child) {
                        cond.items.push(st);
                    }
                }
                IfPhase::Then => {
                    if let Some(st) = lower_statement(src, child) {
                        then_body.items.push(st);
                    }
                }
            },
        }
    }
    Some(Node::If {
        cond: Box::new(cond),
        then_body: Box::new(then_body),
        else_body: else_body.map(Box::new),
    })
}

enum IfPhase {
    Cond,
    Then,
}

/// Lower a word node into fragments. Handles bare words, quoted strings,
/// `$var` / `${var}`, and `$(...)` command substitution.
fn lower_word(src: &str, node: TsNode) -> Word {
    let mut frags = Vec::new();
    collect_frags(src, node, &mut frags);
    if frags.is_empty() {
        // Fallback: treat the raw text as a literal.
        frags.push(Frag::Literal(node_text(src, node).to_string()));
    }
    Word { frags }
}

fn collect_frags(src: &str, node: TsNode, out: &mut Vec<Frag>) {
    match node.kind() {
        // A bare integer arg (`head -n 2`) parses as `number`, not `word`.
        // Treat it as a plain literal token.
        "word" | "number" => out.push(Frag::Literal(node_text(src, node).to_string())),
        "raw_string" => {
            // single-quoted: strip the quotes, never expand.
            let t = node_text(src, node);
            out.push(Frag::Quoted(strip_quotes(t, '\'')));
        }
        "string" => {
            // double-quoted: children carry literal chunks + expansions.
            let mut cursor = node.walk();
            let mut any = false;
            for child in node.children(&mut cursor) {
                match child.kind() {
                    "\"" => {}
                    "string_content" => {
                        // Double-quoted text: literal, NOT glob-eligible.
                        // `echo "*.txt"` must stay `*.txt` (bash semantics).
                        // The `$var` parts inside the quotes are separate
                        // `simple_expansion`/`expansion` children below.
                        out.push(Frag::Quoted(node_text(src, child).to_string()));
                        any = true;
                    }
                    "simple_expansion" | "expansion" => {
                        collect_frags(src, child, out);
                        any = true;
                    }
                    _ => {}
                }
            }
            if !any {
                out.push(Frag::Quoted(strip_quotes(node_text(src, node), '"')));
            }
        }
        "simple_expansion" => {
            // `$name`
            let name = node_text(src, node).trim_start_matches('$');
            out.push(Frag::Var(name.to_string()));
        }
        "expansion" => {
            // `${name}` — grab the variable_name child.
            let mut cursor = node.walk();
            let mut got = false;
            for child in node.children(&mut cursor) {
                if child.kind() == "variable_name" {
                    out.push(Frag::Var(node_text(src, child).to_string()));
                    got = true;
                }
            }
            if !got {
                let inner = node_text(src, node)
                    .trim_start_matches("${")
                    .trim_end_matches('}');
                out.push(Frag::Var(inner.to_string()));
            }
        }
        "command_substitution" => {
            // `$(...)` / backticks — lower the inner commands directly from
            // the CST children rather than re-parsing the node's text. This
            // avoids two traps: (a) `trim_end_matches(')')` would strip ALL
            // trailing parens, corrupting nested `$( $(…) )`; (b) re-parsing
            // recurses the whole parser per nesting level, a stack-overflow
            // vector on pathological input. tree-sitter already parsed the
            // interior — walk it. Depth is bounded by the CST the parser
            // produced (itself length-capped in `parse`).
            let mut inner = Script::default();
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                match child.kind() {
                    "$(" | ")" | "`" => {}
                    _ => {
                        if let Some(st) = lower_statement(src, child) {
                            inner.items.push(st);
                        }
                    }
                }
            }
            out.push(Frag::CmdSub(inner));
        }
        "concatenation" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                collect_frags(src, child, out);
            }
        }
        _ => {}
    }
}

fn strip_quotes(s: &str, q: char) -> String {
    s.strip_prefix(q)
        .and_then(|s| s.strip_suffix(q))
        .unwrap_or(s)
        .to_string()
}

fn is_word_kind(kind: &str) -> bool {
    matches!(
        kind,
        "word"
            | "number"
            | "string"
            | "raw_string"
            | "simple_expansion"
            | "expansion"
            | "command_substitution"
            | "concatenation"
    )
}

fn node_text<'a>(src: &'a str, node: TsNode) -> &'a str {
    node.utf8_text(src.as_bytes()).unwrap_or("")
}
