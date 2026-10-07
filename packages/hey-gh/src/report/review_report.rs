//! The watcher's evidence excludes timeline and review-request history.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrReviewReport {
    pub repository: String,
    pub number: u64,
    pub pull_request: Value,
    pub conflicts: String,
    pub comments: Vec<Value>,
    pub review_comments: Vec<Value>,
    pub reviews: Vec<Value>,
    pub review_threads: Vec<Value>,
    pub review_status: ReviewStatus,
    pub ci: CiReport,
    pub errors: Vec<SourceError>,
}

/// Completeness covers current metadata, CI, comments, reviews and threads.
/// It makes no claim about timeline or review-request history and has no feed
/// cursor: partial reads never replace a full PR snapshot or its source health.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReviewReport {
    pub data: PrReviewReport,
    pub observed_at_ms: u64,
    pub complete: bool,
    pub oldest_validation_at_ms: u64,
    pub validations: Vec<ResourceValidation>,
}

impl From<Report> for ReviewReport {
    fn from(report: Report) -> Self {
        let pr = report.data;
        Self {
            data: PrReviewReport {
                repository: pr.repository,
                number: pr.number,
                pull_request: pr.pull_request,
                conflicts: pr.conflicts,
                comments: pr.comments,
                review_comments: pr.review_comments,
                reviews: pr.reviews,
                review_threads: pr.review_threads,
                review_status: pr.review_status,
                ci: pr.ci,
                errors: pr.errors,
            },
            observed_at_ms: report.observed_at_ms,
            complete: report.complete,
            oldest_validation_at_ms: report.oldest_validation_at_ms,
            validations: report.validations,
        }
    }
}

impl Client {
    pub async fn pr_review_report(
        &self,
        repository: &str,
        number: u64,
        freshness: Freshness,
    ) -> Result<ReviewReport> {
        // Reuse the same ownership, freshness, retries and CI collection as a
        // full read, but never publish the uncollected sources as empty/healthy.
        capture_only(self.scoped_pr_report(repository, number, freshness, ReportScope::Reviews))
            .await
            .map(ReviewReport::from)
    }
}
