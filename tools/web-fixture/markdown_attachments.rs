use super::*;
pub(super) const PROJECT: &str = "Markdown attachments";
pub(super) const CSV: &str = "day,duration\nMonday,24\nTuesday,17\n";
pub(super) fn svg(width: u32) -> String {
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 500 200" role="img" aria-label="Build duration: Monday 24 minutes, Tuesday 17 minutes">
<rect width="500" height="200" rx="16" fill="#172b4d"/>
<g font-family="sans-serif" fill="#ffffff">
<text x="20" y="43" font-size="29" font-weight="600">Build duration</text>
<g font-size="22"><text x="20" y="101">Monday</text><text x="20" y="160">Tuesday</text>
<text x="480" y="101" text-anchor="end">24 min</text><text x="480" y="160" text-anchor="end">17 min</text></g></g>
<rect x="125" y="80" width="260" height="25" rx="5" fill="#48c9b0"/>
<rect x="125" y="139" width="184" height="25" rx="5" fill="#70aaff"/></svg>"##,
        height = width * 2 / 5
    )
}
pub(super) const IMPORT: &[&str] = &[
    "artifact",
    "create",
    "--title",
    "Release evidence",
    "--file",
    "report.md",
    "--request-id",
    "fixture-import",
];
pub(super) fn write_sources(f: &Fixture) -> Result<()> {
    let root = f.root.path();
    fs::write(root.join("chart.svg"), svg(720))?;
    fs::write(root.join("data.csv"), CSV)?;
    fs::write(
        root.join("report.md"),
        "# Release evidence\n\n![Weekly build performance](chart.svg \"Build duration\")\n\n[Download measurements][data]\n\n[data]: data.csv\n\n`![example](missing.png)` remains code.",
    )?;
    Ok(())
}
pub(super) fn remove_sources(f: &Fixture) -> Result<()> {
    for name in ["chart.svg", "data.csv", "report.md"] {
        fs::remove_file(f.root.path().join(name))?;
    }
    Ok(())
}
pub(super) fn seed(f: &Fixture) -> Result<Value> {
    write_sources(f)?;
    let artifact = run(f.command(&[IMPORT, &["--project", PROJECT, "--json"]].concat()))?;
    remove_sources(f)?;
    Ok(json!({"id":artifact["artifact"]["id"],"root":f.root.path()}))
}
pub(super) fn url(base: &str, id: &Value) -> String {
    format!(
        "{}/artifacts#project=named%3AMarkdown%20attachments&artifact={}",
        base.trim_end_matches('/'),
        id.as_str().unwrap()
    )
}
