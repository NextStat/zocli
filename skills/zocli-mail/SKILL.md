---
name: zocli-mail
description: "Zoho Mail workflow: list folders, inspect messages, search, send, reply, and forward. Use it for any stable mail task that zocli currently supports."
metadata:
  author: NextStat
---

# zocli mail

Read `zocli-shared` first if account setup or auth state is unclear.

## Commands

### List folders

```
zocli mail folders [--profile ALIAS]
```

### List messages

```
zocli mail list [--folder-id FOLDER_ID] [--unread] [--limit N] [--profile ALIAS]
```

Without `--folder-id`, `zocli` uses the Inbox. The command returns normalized message summaries.

### Search messages

```
zocli mail search QUERY [--limit N] [--profile ALIAS]
```

Search returns the same summary surface used by `mail list`.

### Read one message

```
zocli mail read --folder-id FOLDER_ID MESSAGE_ID [--profile ALIAS]
```

You need both the folder ID and the message ID returned by `mail list` or `mail search`.

### Send a message

```
zocli mail send TO SUBJECT [BODY] [--cc EMAIL]... [--bcc EMAIL]... [--html HTML] [--profile ALIAS]
```

Text and HTML bodies are supported. Attachments are not part of the stable public surface.

### Reply to a message

```
zocli mail reply --folder-id FOLDER_ID MESSAGE_ID [BODY] [--cc EMAIL]... [--html HTML] [--profile ALIAS]
```

`zocli` preserves the reply chain automatically.

### Forward a message

```
zocli mail forward --folder-id FOLDER_ID MESSAGE_ID TO [BODY] [--cc EMAIL]... [--bcc EMAIL]... [--html HTML] [--profile ALIAS]
```

Forwarding is message-level only. Attachment export is not part of the current stable surface.

## Typical flow

1. `zocli mail folders` to discover folder IDs when needed.
2. `zocli mail list` or `zocli mail search` to discover the message ID.
3. `zocli mail read --folder-id ...` to inspect the message.
4. `zocli mail reply ...` or `zocli mail forward ...` to act on it.
