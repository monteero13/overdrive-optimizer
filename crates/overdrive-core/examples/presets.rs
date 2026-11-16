//! Community presets aggregation, run by `.github/workflows/presets.yml`:
//!
//!   cargo run -p overdrive-core --example presets -- issues.json presets/community.json
//!
//! Reads issues as `gh api` returns them (`[{"title": …, "body": …}]`), keeps the
//! measurement issues, verifies every signature, counts each install once and writes the
//! presets file the app downloads. Pure and cross-platform (runs on a Linux runner).

use overdrive_core::share::{aggregate_issue_bodies, ISSUE_MARKER};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(input), Some(output)) = (args.first(), args.get(1)) else {
        eprintln!("usage: presets <issues.json> <out.json> [YYYY-MM-DD]");
        std::process::exit(2);
    };
    let text = std::fs::read_to_string(input).unwrap_or_else(|e| {
        eprintln!("error: {input}: {e}");
        std::process::exit(1);
    });
    // `gh api --paginate` concatenates one JSON array per page: accept both forms.
    let issues: Vec<serde_json::Value> = serde_json::Deserializer::from_str(&text)
        .into_iter::<Vec<serde_json::Value>>()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|e| {
            eprintln!("error: {input} is not JSON arrays of issues: {e}");
            std::process::exit(1);
        })
        .into_iter()
        .flatten()
        .collect();
    let bodies: Vec<String> = issues
        .iter()
        .filter(|i| i["title"].as_str().is_some_and(|t| t.starts_with(ISSUE_MARKER)))
        .filter(|i| i.get("pull_request").is_none())
        .filter_map(|i| i["body"].as_str().map(String::from))
        .collect();
    let day = args.get(2).cloned().unwrap_or_else(|| "unknown".into());
    let file = aggregate_issue_bodies(&bodies, &day);
    let json = serde_json::to_string_pretty(&file).expect("serialises") + "\n";
    std::fs::write(output, json).unwrap_or_else(|e| {
        eprintln!("error: {output}: {e}");
        std::process::exit(1);
    });
    println!("{} measurement issues → {} presets", bodies.len(), file.presets.len());
}
