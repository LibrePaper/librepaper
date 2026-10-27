//! Walking a paged listing: the keyset cursor the document listing hands out
//! and the loop that follows it to the end.

use std::collections::HashSet;
use std::future::Future;
use std::hash::Hash;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The most pages a client follows before it gives up. A page holds at most
/// 200 documents, so this is the 20,000 projects a backup accepts.
pub const LISTING_MAX_PAGES: usize = 100;

/// The `next_cursor` of the document listing. The browser and the clients
/// send it back as `after_updated` and `after_slug` query fields.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ListingCursor {
    pub after_updated: String,
    pub after_slug: String,
}

impl ListingCursor {
    /// The cursor a request carries in its query, when it has both fields.
    pub fn from_query<'a>(mut get: impl FnMut(&str) -> Option<&'a str>) -> Option<Self> {
        Some(Self {
            after_updated: get("after_updated")?.to_string(),
            after_slug: get("after_slug")?.to_string(),
        })
    }

    /// The cursor in a listing response, if there is a next page. A cursor
    /// that is present but malformed is an error.
    pub fn from_payload(payload: &Value) -> Result<Option<Self>, serde_json::Error> {
        match payload.get("next_cursor").filter(|value| !value.is_null()) {
            Some(value) => serde_json::from_value(value.clone()).map(Some),
            None => Ok(None),
        }
    }

    /// The cursor as the `next_cursor` value of a response.
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).expect("a cursor of two strings encodes")
    }

    /// Replaces the query of `target` with this cursor.
    pub fn apply(&self, target: &mut url::Url) {
        target
            .query_pairs_mut()
            .clear()
            .append_pair("after_updated", &self.after_updated)
            .append_pair("after_slug", &self.after_slug);
    }
}

/// One fetched page and the cursor of the page after it, if any.
pub struct Page<T, K> {
    pub items: Vec<T>,
    pub next: Option<K>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum WalkError<E> {
    /// The fetch itself failed.
    Fetch(E),
    /// A cursor came back that an earlier page had already returned.
    Repeated,
    /// More pages were asked for than the bound allows.
    TooManyPages,
}

/// Fetches pages from the first until one has no next cursor, and returns
/// every item in order. The first fetch gets `None`.
pub async fn walk<T, K, F, Fut, E>(
    mut fetch: F,
    max_pages: usize,
) -> Result<Vec<T>, WalkError<E>>
where
    K: Eq + Hash + Clone,
    F: FnMut(Option<K>) -> Fut,
    Fut: Future<Output = Result<Page<T, K>, E>>,
{
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    let mut cursor: Option<K> = None;
    let mut fetched = 0;
    loop {
        if fetched >= max_pages {
            return Err(WalkError::TooManyPages);
        }
        fetched += 1;
        let page = fetch(cursor.take()).await.map_err(WalkError::Fetch)?;
        items.extend(page.items);
        let Some(next) = page.next else {
            return Ok(items);
        };
        if !seen.insert(next.clone()) {
            return Err(WalkError::Repeated);
        }
        cursor = Some(next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_cursor_keeps_its_wire_fields() {
        let cursor = ListingCursor {
            after_updated: "2026-01-02T03:04:05Z".into(),
            after_slug: "abc".into(),
        };
        let wire = json!({"after_updated": "2026-01-02T03:04:05Z", "after_slug": "abc"});
        assert_eq!(cursor.to_json(), wire);
        let payload = json!({"documents": [], "next_cursor": wire});
        assert_eq!(ListingCursor::from_payload(&payload).unwrap(), Some(cursor));
        assert_eq!(
            ListingCursor::from_payload(&json!({"next_cursor": null})).unwrap(),
            None
        );
        assert!(ListingCursor::from_payload(&json!({"next_cursor": {"after_slug": "x"}})).is_err());
    }

    #[test]
    fn the_query_needs_both_fields() {
        let both = |name: &str| match name {
            "after_updated" => Some("u"),
            "after_slug" => Some("s"),
            _ => None,
        };
        assert_eq!(
            ListingCursor::from_query(both),
            Some(ListingCursor {
                after_updated: "u".into(),
                after_slug: "s".into()
            })
        );
        assert_eq!(
            ListingCursor::from_query(|name| (name == "after_slug").then_some("s")),
            None
        );
    }

    fn pages(next: &'static [Option<u32>]) -> impl FnMut(Option<u32>) -> std::future::Ready<Result<Page<u32, u32>, ()>> {
        let mut index = 0;
        move |_| {
            let page = Page {
                items: vec![index as u32],
                next: next[index],
            };
            index += 1;
            std::future::ready(Ok(page))
        }
    }

    #[tokio::test]
    async fn walk_collects_every_page() {
        let items = walk(pages(&[Some(1), Some(2), None]), 10).await.unwrap();
        assert_eq!(items, vec![0, 1, 2]);
    }

    #[tokio::test]
    async fn walk_stops_on_a_repeated_cursor() {
        let result = walk(pages(&[Some(1), Some(2), Some(1), None]), 10).await;
        assert_eq!(result, Err(WalkError::Repeated));
    }

    #[tokio::test]
    async fn walk_stops_at_the_page_bound() {
        let result = walk(pages(&[Some(1), Some(2), Some(3), None]), 3).await;
        assert_eq!(result, Err(WalkError::TooManyPages));
        let items = walk(pages(&[Some(1), Some(2), None]), 3).await.unwrap();
        assert_eq!(items.len(), 3);
    }

    #[tokio::test]
    async fn walk_reports_a_failed_fetch() {
        let result: Result<Vec<u32>, _> = walk(
            |_: Option<u32>| std::future::ready(Err::<Page<u32, u32>, _>("down")),
            3,
        )
        .await;
        assert_eq!(result, Err(WalkError::Fetch("down")));
    }
}
