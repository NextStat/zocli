use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;

#[test]
fn homebrew_formula_includes_linux_arm64_release_asset() {
    let temp = tempdir().expect("tempdir");
    let output = temp.path().join("zocli.rb");

    let status = Command::new("sh")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args([
            "./scripts/generate-homebrew-formula.sh",
            "--version",
            "0.2.0",
            "--base-url",
            "https://example.com/releases",
            "--output",
            output.to_str().expect("utf8 path"),
            "--sha256-linux-arm64",
            "arm64sha",
            "--sha256-linux-x64",
            "x64sha",
            "--sha256-macos-arm64",
            "macarm64sha",
        ])
        .status()
        .expect("formula script runs");

    assert!(status.success());

    let formula = fs::read_to_string(&output).expect("formula");
    assert!(formula.contains("zocli-aarch64-unknown-linux-gnu.tar.gz"));
    assert!(formula.contains("sha256 \"arm64sha\""));
    assert!(formula.contains("elsif Hardware::CPU.intel?"));
    assert!(formula.contains("sha256 \"x64sha\""));
}

// ── Parity Manifest ──────────────────────────────────────
//
// This test is the machine-readable product surface snapshot
// required by the MCP Apps ADR (Phase 0).
//
// If you add or remove a tool, prompt, skill, or resource,
// update the corresponding constant below. If a new entry
// appears on disk but isn't registered, the test fails.

/// All MCP tools that are advertised at runtime.
const EXPECTED_MCP_TOOLS: &[&str] = &[
    "zocli.app.snapshot",
    "zocli.roots.list",
    "zocli.account.list",
    "zocli.account.current",
    "zocli.auth.status",
    "zocli.update.check",
    "zocli.mail.folders",
    "zocli.mail.list",
    "zocli.mail.search",
    "zocli.mail.read",
    "zocli.mail.send",
    "zocli.mail.reply",
    "zocli.mail.forward",
    "zocli.mail.attachment_export",
    "zocli.calendar.calendars",
    "zocli.calendar.events",
    "zocli.calendar.create",
    "zocli.calendar.delete",
    "zocli.drive.teams",
    "zocli.drive.list",
    "zocli.drive.upload",
    "zocli.drive.download",
];

/// All MCP prompts exposed at runtime.
const EXPECTED_PROMPTS: &[&str] = &[
    "shared",
    "mail",
    "calendar",
    "drive",
    "daily-briefing",
    "find-and-read",
    "reply-with-context",
];

/// All skills registered in skills.rs and deployed via `mcp install`.
const EXPECTED_SKILLS: &[&str] = &[
    "zocli-shared",
    "zocli-mail",
    "zocli-calendar",
    "zocli-drive",
    "zocli-daily-briefing",
    "zocli-find-and-read",
    "zocli-reply-with-context",
];

/// All MCP app resources (ui:// URIs).
const EXPECTED_APP_RESOURCES: &[&str] = &[
    "ui://zocli/dashboard",
    "ui://zocli/mail",
    "ui://zocli/calendar",
    "ui://zocli/drive",
    "ui://zocli/auth",
    "ui://zocli/account",
];

#[test]
fn parity_manifest_skills_match_disk() {
    let skills_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("skills");
    let on_disk: BTreeSet<String> = fs::read_dir(&skills_dir)
        .expect("skills directory")
        .filter_map(|e| {
            let e = e.ok()?;
            if e.file_type().ok()?.is_dir() {
                Some(e.file_name().to_string_lossy().to_string())
            } else {
                None
            }
        })
        .collect();

    let registered: BTreeSet<String> = EXPECTED_SKILLS.iter().map(|s| s.to_string()).collect();

    let orphaned: Vec<_> = on_disk.difference(&registered).collect();
    assert!(
        orphaned.is_empty(),
        "Orphaned skill directories on disk (not registered in skills.rs): {orphaned:?}. \
         Either register them or delete the directories."
    );

    let missing: Vec<_> = registered.difference(&on_disk).collect();
    assert!(
        missing.is_empty(),
        "Skills registered but missing on disk: {missing:?}"
    );
}

#[test]
fn parity_manifest_counts_are_consistent() {
    assert_eq!(
        EXPECTED_MCP_TOOLS.len(),
        22,
        "MCP tools count changed — update EXPECTED_MCP_TOOLS"
    );
    assert_eq!(
        EXPECTED_PROMPTS.len(),
        7,
        "Prompts count changed — update EXPECTED_PROMPTS"
    );
    assert_eq!(
        EXPECTED_SKILLS.len(),
        7,
        "Skills count changed — update EXPECTED_SKILLS"
    );
    assert_eq!(
        EXPECTED_APP_RESOURCES.len(),
        6,
        "App resources count changed — update EXPECTED_APP_RESOURCES"
    );
}
