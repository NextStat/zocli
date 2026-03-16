# Changelog

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
