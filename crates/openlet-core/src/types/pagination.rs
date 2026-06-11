//! Cursor pagination primitives for unbounded list methods.
//!
//! Local stores hold few enough rows to return a `Vec`, but a cloud
//! Postgres backend serving many tenants needs bounded pages. These
//! types are the trait-level contract: a [`Page`] request (opaque
//! cursor + limit) in, a [`PageResult`] (items + next cursor) out.
//!
//! The cursor is an **opaque** string — callers pass back exactly what
//! the store returned and never parse it. The local default impl encodes
//! a row offset; a cloud impl may encode a keyset position. `None`
//! `next_cursor` means the last page was returned.

use serde::{Deserialize, Serialize};

/// Default page size when a caller does not specify one.
pub const DEFAULT_PAGE_LIMIT: u32 = 50;

/// A page request: where to resume (`cursor`) and how many rows
/// (`limit`). An absent cursor starts from the beginning.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Page {
    /// Opaque resume token from a prior [`PageResult::next_cursor`].
    /// `None` → start at the first row.
    pub cursor: Option<String>,
    /// Max rows to return. `0` is treated as [`DEFAULT_PAGE_LIMIT`] by
    /// the helper so a default-constructed `Page` is usable.
    pub limit: u32,
}

impl Page {
    /// Build a first-page request with an explicit limit.
    #[must_use]
    pub fn first(limit: u32) -> Self {
        Self {
            cursor: None,
            limit,
        }
    }

    /// The effective limit, substituting [`DEFAULT_PAGE_LIMIT`] for `0`.
    #[must_use]
    pub fn effective_limit(&self) -> u32 {
        if self.limit == 0 {
            DEFAULT_PAGE_LIMIT
        } else {
            self.limit
        }
    }
}

/// One page of results plus the cursor to fetch the next page. When
/// `next_cursor` is `None`, the caller has reached the end.
#[derive(Debug, Clone)]
pub struct PageResult<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

impl<T> PageResult<T> {
    /// Slice an already-materialized `Vec` into a page using an
    /// offset-encoded cursor. This is the bridge the default trait
    /// methods use to paginate over stores that only expose unbounded
    /// `list_*` — a cloud store overrides the paged method with native
    /// `LIMIT/OFFSET` (or keyset) SQL instead.
    ///
    /// Cursor format: decimal row offset. A malformed cursor is treated
    /// as offset 0 (fail-open to the first page rather than erroring on
    /// an opaque token the caller was told not to inspect).
    #[must_use]
    pub fn from_slice(all: Vec<T>, page: &Page) -> Self {
        let limit = page.effective_limit() as usize;
        let offset = page
            .cursor
            .as_deref()
            .and_then(|c| c.parse::<usize>().ok())
            .unwrap_or(0);

        let total = all.len();
        let end = offset.saturating_add(limit).min(total);
        let start = offset.min(total);

        let items: Vec<T> = all.into_iter().skip(start).take(end - start).collect();
        let next_cursor = if end < total {
            Some(end.to_string())
        } else {
            None
        };
        Self { items, next_cursor }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_page_then_next() {
        let all: Vec<u32> = (0..10).collect();
        let p = PageResult::from_slice(all.clone(), &Page::first(3));
        assert_eq!(p.items, vec![0, 1, 2]);
        assert_eq!(p.next_cursor.as_deref(), Some("3"));

        let p2 = PageResult::from_slice(
            all,
            &Page {
                cursor: p.next_cursor,
                limit: 3,
            },
        );
        assert_eq!(p2.items, vec![3, 4, 5]);
        assert_eq!(p2.next_cursor.as_deref(), Some("6"));
    }

    #[test]
    fn last_page_has_no_cursor() {
        let all: Vec<u32> = (0..5).collect();
        let p = PageResult::from_slice(
            all,
            &Page {
                cursor: Some("3".into()),
                limit: 10,
            },
        );
        assert_eq!(p.items, vec![3, 4]);
        assert_eq!(p.next_cursor, None);
    }

    #[test]
    fn zero_limit_uses_default() {
        assert_eq!(Page::default().effective_limit(), DEFAULT_PAGE_LIMIT);
    }

    #[test]
    fn offset_past_end_yields_empty() {
        let all: Vec<u32> = (0..3).collect();
        let p = PageResult::from_slice(
            all,
            &Page {
                cursor: Some("99".into()),
                limit: 5,
            },
        );
        assert!(p.items.is_empty());
        assert_eq!(p.next_cursor, None);
    }

    #[test]
    fn malformed_cursor_falls_back_to_first_page() {
        let all: Vec<u32> = (0..4).collect();
        let p = PageResult::from_slice(
            all,
            &Page {
                cursor: Some("not-a-number".into()),
                limit: 2,
            },
        );
        assert_eq!(p.items, vec![0, 1]);
        assert_eq!(p.next_cursor.as_deref(), Some("2"));
    }
}
