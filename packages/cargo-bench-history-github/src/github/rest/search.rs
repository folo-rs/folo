use std::collections::HashSet;
use std::num::NonZero;

use ohno::AppError;
use reqwest::Method;
use serde::Deserialize;
use serde_json::Value;

use crate::errors::InvalidResponseError;
use crate::github::IssueCandidate;
use crate::github::http::Http;
use crate::github::rest::{PaginationError, RestGitHub};
use crate::model::Repository;

impl<H: Http> RestGitHub<H> {
    pub(crate) async fn search(
        &self,
        repository: &Repository,
        phrase: &str,
        include_closed: bool,
    ) -> Result<Vec<IssueCandidate>, AppError> {
        // GitHub's search API exposes at most this many results. At the boundary we
        // cannot prove completeness, so capped discovery never authorizes creation.
        const SEARCH_CAP: usize = 1000;
        const FIRST_PAGE: u64 = 1;
        let operation = "searching issue titles";
        let query = title_query(repository, phrase, include_closed);
        let mut page = FIRST_PAGE;
        let mut total = None;
        let mut candidates = Vec::new();
        let mut seen = HashSet::new();
        loop {
            let mut request = self.request(Method::GET, repository, "")?;
            request.url_mut().set_path("/search/issues");
            request
                .url_mut()
                .query_pairs_mut()
                .append_pair("q", &query)
                .append_pair("per_page", &self.page_size.to_string())
                .append_pair("page", &page.to_string());
            let response: SearchResponse = self.send_json(operation, request).await?;
            if response.incomplete_results || response.total_count >= SEARCH_CAP {
                return Err(InvalidResponseError::new(operation).into());
            }
            if total.is_some_and(|count| count != response.total_count)
                || response.items.len() > self.page_size.get()
                || response.items.iter().any(|item| !seen.insert(item.number))
            {
                return Err(PaginationError::new(operation).into());
            }
            if response
                .items
                .iter()
                .any(|item| item.pull_request.is_some())
            {
                return Err(InvalidResponseError::new(operation).into());
            }
            total = Some(response.total_count);
            let last_page = response.items.len() < self.page_size.get();
            candidates.extend(response.items.into_iter().map(|item| IssueCandidate {
                number: item.number.get(),
                title: item.title,
            }));
            if candidates.len() > response.total_count
                || (last_page && candidates.len() != response.total_count)
            {
                return Err(PaginationError::new(operation).into());
            }
            if last_page {
                return Ok(candidates);
            }
            page = page
                .checked_add(1)
                .ok_or_else(|| PaginationError::new(operation))?;
        }
    }
}

fn title_query(repository: &Repository, phrase: &str, include_closed: bool) -> String {
    // Escape the query language before URL encoding. Project text is a literal phrase,
    // never additional search qualifiers; the date is intentionally not part of identity.
    let phrase = phrase.replace('\\', "\\\\").replace('"', "\\\"");
    let state = if include_closed { "" } else { " is:open" };
    format!("repo:{repository} is:issue{state} in:title \"{phrase}\"")
}

/// Search completeness is a prerequisite for any empty-result interpretation.
#[derive(Deserialize)]
struct SearchResponse {
    total_count: usize,
    incomplete_results: bool,
    items: Vec<SearchItem>,
}

/// Indexed bodies and states are intentionally excluded from candidate data.
#[derive(Deserialize)]
struct SearchItem {
    number: NonZero<u64>,
    title: String,
    pull_request: Option<Value>,
}
