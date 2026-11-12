//! CI gate for the offline deterministic evaluation suite.

use std::path::Path;

use canvas_agent::eval::run_suite;

#[tokio::test]
async fn all_eval_scenarios_pass() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("evals/scenarios.json");
    let report = run_suite(&path).await.expect("eval suite runs");
    report.print();
    assert!(
        report.all_passed(),
        "{} of {} evaluation scenarios failed",
        report.failed(),
        report.results.len()
    );
}
