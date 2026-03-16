---
name: zocli-mail
description: "Zoho Mail workflow for English and Russian requests: list folders, inspect messages, search, send, reply, and forward. Zoho Mail workflow: list folders, inspect messages, search, send, reply, and forward. Также используй для русских запросов по почте."
metadata:
  author: NextStat
---

# zocli mail

Use this skill for both English and Russian mail requests.
Используй этот скилл и для английских, и для русских запросов по почте.

Read `zocli-shared` first if account setup or auth state is unclear.

Сначала прочитай `zocli-shared`, если неясны настройки аккаунта или статус авторизации.

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

Без `--folder-id` используется Inbox. Команда возвращает нормализованные summaries писем.

### Search messages

```
zocli mail search QUERY [--limit N] [--profile ALIAS]
```

Search returns the same summary surface used by `mail list`.

Поиск возвращает тот же summary surface, что и `mail list`.

### Read one message

```
zocli mail read --folder-id FOLDER_ID MESSAGE_ID [--profile ALIAS]
```

You need both the folder ID and the message ID returned by `mail list` or `mail search`.

Нужны и `folder_id`, и `message_id`, которые вернули `mail list` или `mail search`.

### Send a message

```
zocli mail send TO SUBJECT [BODY] [--cc EMAIL]... [--bcc EMAIL]... [--html HTML] [--profile ALIAS]
```

Text and HTML bodies are supported. Attachments are not part of the stable public surface.

Поддерживаются text и HTML. Вложения не входят в текущую stable public surface.

### Reply to a message

```
zocli mail reply --folder-id FOLDER_ID MESSAGE_ID [BODY] [--cc EMAIL]... [--html HTML] [--profile ALIAS]
```

`zocli` preserves the reply chain automatically.

`zocli` автоматически сохраняет цепочку ответа.

### Forward a message

```
zocli mail forward --folder-id FOLDER_ID MESSAGE_ID TO [BODY] [--cc EMAIL]... [--bcc EMAIL]... [--html HTML] [--profile ALIAS]
```

Forwarding is message-level only. Attachment export is not part of the current stable surface.

Forward работает только на уровне сообщения. Экспорт вложений не входит в текущую stable surface.

## Typical flow

1. `zocli mail folders` to discover folder IDs when needed.
2. `zocli mail list` or `zocli mail search` to discover the message ID.
3. `zocli mail read --folder-id ...` to inspect the message.
4. `zocli mail reply ...` or `zocli mail forward ...` to act on it.
5. If the user asked in Russian, answer in Russian, but keep command names and identifiers unchanged.
6. Если пользователь пишет по-русски, отвечай по-русски, но команды и идентификаторы не переводи.
