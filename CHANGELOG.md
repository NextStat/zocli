# Changelog

## 0.2.4

### Changed

- MCP Apps now ship with browser-backed conformance coverage in the release gate, using a real Chromium iframe and `postMessage` lifecycle instead of HTML-shape-only checks.
- CI now treats browser-backed MCP Apps verification as mandatory, so release candidates cannot silently skip the Chromium-based conformance suite.

### Fixed

- `zocli mcp install --client gemini` now uses the real Gemini CLI stdio registration shape and succeeds against the live Gemini host instead of failing on an invalid `mcp add` argument form.
- real-host MCP install verification now covers Claude, Codex, Gemini, and Cursor registration paths against live local surfaces instead of only stubbed CLI scripts.

## 0.2.3

### Changed

- MCP Apps tool results now return concise human summaries in `content` for app-capable hosts, while keeping full machine data in `structuredContent`.

### Fixed

- non-UI MCP clients still receive raw JSON tool payloads instead of summary-only text;
- JSON `resources/read` payloads keep machine-readable `application/json` content instead of app-style summaries;
- MCP HTTP SSE tests now use a dedicated streaming client timeout, removing flaky notification timeouts from the release gate.

## 0.2.2

### Changed

- CLI output now defaults to `auto`: terminals get human-readable output, while pipes and scripts keep JSON by default.
- `zocli update --check` and `zocli update` now print concise human messages in TTY sessions instead of raw field dumps.

### Fixed

- MCP HTTP test harness now retries ephemeral port binding instead of failing on transient `Address already in use` races.

## 0.2.1

### Fixed

- `zocli update --check` and `zocli update` now resolve GitHub `releases/latest/download` to the concrete published version instead of reporting `target_version: "latest"`;
- update status now reports `already_up_to_date` correctly for current binaries when the latest published release matches the installed version;
- MCP `zocli.update.check` now inherits the same concrete version resolution path as the CLI update surface.

## 0.2.0

### Added

- focused MCP App surfaces for `mail`, `calendar`, `drive`, `auth`, and `account`;
- stable attachment support for `zocli.mail.send` via CLI `--attachment` and MCP `attachments`;
- stable MCP mail attachment export via `zocli.mail.attachment_export`;
- versioned MCP structured payloads with `schemaVersion: "1.0"` and expanded schema coverage.

### Changed

- simplified onboarding: `zocli add <email>` now works with the shared/default zocli OAuth app;
- `account_id` is auto-discovered after the first successful login instead of being required up front;
- non-`com` Zoho accounts are now documented through explicit `--datacenter` setup;
- MCP Apps lifecycle now enforces `ui/initialize` and resets correctly on `ui/resource-teardown`;
- `yacli` and `zocli` now coexist during MCP client installation instead of one replacing the other.

### Fixed

- README, skills, prompts, AGENTS, and release/install/update docs now match the live Zoho runtime surface;
- MCP Apps branding and dashboard copy are aligned with the actual hosted app surface.
