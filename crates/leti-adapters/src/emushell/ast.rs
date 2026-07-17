//! Our own typed shell AST.
//!
//! tree-sitter gives a stringly-typed CST (every node kind is a `&str`).
//! Walking that directly in the evaluator would scatter string matches
//! across the eval loop. Instead `parse.rs` lowers the CST into these
//! typed nodes once, and `eval.rs` walks a closed enum — the same
//! separation an AST-typed parser would give, but with a parser we do
//! not couple to.

/// A full parsed script: a sequence of and/or lists run top to bottom.
#[derive(Debug, Clone, Default)]
pub struct Script {
    pub items: Vec<AndOr>,
}

/// A pipeline plus any `&&` / `||` continuations. `bash` runs the head
/// pipeline, then each continuation only if the running exit status
/// permits (`&&` needs 0, `||` needs non-0).
#[derive(Debug, Clone)]
pub struct AndOr {
    pub head: Node,
    pub tail: Vec<(AndOrOp, Node)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AndOrOp {
    And,
    Or,
}

/// One executable unit in the tree.
#[derive(Debug, Clone)]
pub enum Node {
    /// `a | b | c` — a chain of simple commands wired stdout→stdin.
    Pipeline(Vec<Command>),
    /// `for x in words; do body; done`
    For {
        var: String,
        items: Vec<Word>,
        body: Box<Script>,
    },
    /// `while cond; do body; done`
    While {
        cond: Box<Script>,
        body: Box<Script>,
    },
    /// `if cond; then body; [else else_body]; fi`
    If {
        cond: Box<Script>,
        then_body: Box<Script>,
        else_body: Option<Box<Script>>,
    },
}

/// A simple command: argv words plus redirections.
#[derive(Debug, Clone, Default)]
pub struct Command {
    pub words: Vec<Word>,
    /// Output redirect target + whether it appends (`>>` vs `>`).
    pub redirect_out: Option<(Word, bool)>,
    /// Input redirect source (`< file`).
    pub redirect_in: Option<Word>,
    /// `VAR=value` assignments that prefix the command (or stand alone).
    pub assignments: Vec<(String, Word)>,
}

/// A word before expansion — a list of fragments that concatenate.
/// Kept as fragments so `"$a"b` expands the var but keeps the literal
/// tail, and so glob metacharacters in a literal fragment are only
/// honored when unquoted.
#[derive(Debug, Clone, Default)]
pub struct Word {
    pub frags: Vec<Frag>,
}

/// One piece of a word.
#[derive(Debug, Clone)]
pub enum Frag {
    /// Unquoted or double-quoted literal text. Glob metacharacters here
    /// are eligible for expansion (see `Word::has_glob`).
    Literal(String),
    /// Single-quoted literal — never glob-expanded, never re-split.
    Quoted(String),
    /// `$name` / `${name}` — substituted from the environment.
    Var(String),
    /// `$(...)` / backticks — command substitution, re-enters eval.
    CmdSub(Script),
}
