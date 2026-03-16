---
name: zocli-shared
description: "Shared zocli context for Zoho Mail, Zoho Calendar, and Zoho WorkDrive. Also use it for Russian requests about accounts, auth, output format, and command discovery. Общий контекст zocli: аккаунты, авторизация, форматы вывода и навигация по командам."
metadata:
  author: NextStat
---

# zocli shared surface

Use this skill for both English and Russian requests.
Используй этот скилл и для английских, и для русских запросов.

`zocli` is a CLI for Zoho Mail, Zoho Calendar, and Zoho WorkDrive. By default, commands return JSON.

`zocli` работает с Zoho Mail, Zoho Calendar и Zoho WorkDrive. По умолчанию команды возвращают JSON.

## Output format

Success responses follow `{"ok": true, ...}`. Errors follow `{"ok": false, "code": "...", "message": "..."}`.

Use `--format table` for a human-readable view.

Используй `--format table`, когда нужен человекочитаемый вывод.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Success |
| 1 | User error such as validation, auth, or not found |
| 4 | Config or filesystem error |
| 5 | Network or upstream API error |

## Account management

`zocli` supports multiple named accounts.

| Command | Description |
| --- | --- |
| `zocli add <EMAIL> [ALIAS] --account-id <ID> --client-id <CLIENT_ID>` | Add an account |
| `zocli accounts` | List configured accounts |
| `zocli use <ALIAS>` | Switch the current account |
| `zocli whoami` | Show the current account |
| `zocli status [--profile ALIAS]` | Show auth state for one account |

## Authentication

Each service authenticates through Zoho OAuth2.

| Command | Description |
| --- | --- |
| `zocli login [SERVICE]` | Authenticate mail, calendar, drive, or all of them |
| `zocli logout [SERVICE] [--profile ALIAS]` | Revoke one service or all services |

`SERVICE` is one of `mail`, `calendar`, or `drive`. Without an argument, `zocli login` authenticates all three.

`SERVICE` может быть `mail`, `calendar` или `drive`. Без аргумента `zocli login` авторизует все три сервиса.

## Global flags

- `--format json|table` controls output rendering
- `--profile ALIAS` overrides the current account on service commands
- `-h, --help` shows help
- `-V, --version` shows the version

## Discovery

Use `zocli guide` when you need the stable command catalog:

Используй `zocli guide`, когда нужен стабильный каталог команд:

```bash
zocli guide
zocli guide --topic mail
zocli guide --topic drive
```

## Multiple accounts

Use `--profile ALIAS` when the current account is not the one you want:

Используй `--profile ALIAS`, когда нужен не текущий аккаунт:

```
zocli mail list --profile work
zocli calendar events --profile personal
```
