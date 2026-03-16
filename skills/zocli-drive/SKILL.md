---
name: zocli-drive
description: "Zoho WorkDrive workflow: inspect teams, browse folders, upload files, and download files. Use it for the stable WorkDrive file surface."
metadata:
  author: NextStat
---

# zocli drive

Read `zocli-shared` first if account setup or auth state is unclear.

## Commands

### Show WorkDrive info

```
zocli drive info [--profile ALIAS]
```

Shows teams, storage information, and related WorkDrive metadata.

### List teams or folder contents

```
zocli drive list [FOLDER_ID] [--limit N] [--offset N] [--profile ALIAS]
```

Without `FOLDER_ID`, the command lists available WorkDrive teams. With a folder ID, it lists that folder's contents.

Example:
```
zocli drive list 4a6xxxxxxxxxxxxxx --limit 50
```

### Upload a file

```
zocli drive upload FILE FOLDER_ID [--overwrite] [--profile ALIAS]
```

Example:
```
zocli drive upload ./report.pdf 4a6xxxxxxxxxxxxxx --overwrite
```

### Download a file

```
zocli drive download FILE_ID --output ./report.pdf [--force] [--profile ALIAS]
```

Use the file ID returned by `drive list` or another WorkDrive surface.

## Typical flow

1. `zocli drive info` to inspect the account-level WorkDrive context.
2. `zocli drive list` to discover teams or a folder ID.
3. `zocli drive upload ...` or `zocli drive download ...` for file transfer.
