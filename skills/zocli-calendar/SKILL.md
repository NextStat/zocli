---
name: zocli-calendar
description: "Zoho Calendar workflow for English and Russian requests: list calendars, inspect events, create meetings, and delete events. Также используй для русских запросов про встречи, расписание и доступность."
metadata:
  author: NextStat
---

# zocli calendar

Use this skill for both English and Russian calendar requests.
Используй этот скилл и для английских, и для русских запросов по календарю.

Read `zocli-shared` first if account setup or auth state is unclear.

Сначала прочитай `zocli-shared`, если неясны настройки аккаунта или статус авторизации.

## Commands

### List calendars

```
zocli calendar calendars [--profile ALIAS]
```

Returns the available calendars with their names and IDs.

### List events

```
zocli calendar events [FROM] [TO] [--calendar CALENDAR_UID] [--limit N] [--profile ALIAS]
```

`FROM` and `TO` accept `YYYY-MM-DD`. Without explicit bounds, `zocli` defaults to the next 30 days.

`FROM` и `TO` принимают `YYYY-MM-DD`. Без явных границ `zocli` по умолчанию смотрит на ближайшие 30 дней.

Example:
```
zocli calendar events 2026-03-16 2026-03-23 --limit 20
```

### Create an event

```
zocli calendar create TITLE START END [--calendar CALENDAR_UID] [--description TEXT] [--location PLACE] [--profile ALIAS]
```

`START` and `END` use RFC3339 timestamps such as `2026-03-16T09:00:00Z`.

`START` и `END` используют RFC3339 timestamps, например `2026-03-16T09:00:00Z`.

Example:
```
zocli calendar create "Team Sync" 2026-03-16T09:00:00Z 2026-03-16T10:00:00Z --description "Weekly sync"
```

### Delete an event

```
zocli calendar delete UID [--calendar CALENDAR_UID] [--profile ALIAS]
```

Use the event UID returned by `calendar events`.

Используй `UID`, который вернул `calendar events`.

## Typical flow

1. Run `zocli calendar calendars` to discover the target calendar.
2. Run `zocli calendar events` with the narrowest useful window.
3. Run `zocli calendar create ...` or `zocli calendar delete ...`.
4. If the request is in Russian, keep the final explanation in Russian, but do not translate command names or IDs.
