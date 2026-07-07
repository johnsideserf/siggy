> This is the Russian translation of the siggy README.
> Last updated against English commit: 8b5890e
> The [English version](../README.md) is authoritative. If this translation has drifted, trust the English.
> This translation is maintainer-provided and awaiting native-speaker review. Corrections welcome - see issue #353.

<p align="center">
  <img src="../siggy-banner.png" alt="siggy" width="600">
</p>

<p align="center">
  <a href="https://github.com/johnsideserf/siggy/actions/workflows/ci.yml"><img src="https://github.com/johnsideserf/siggy/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/johnsideserf/siggy/releases/latest"><img src="https://img.shields.io/github/v/release/johnsideserf/siggy" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/johnsideserf/siggy" alt="License: AGPL-3.0"></a>
  <a href="https://crates.io/crates/siggy"><img src="https://img.shields.io/crates/v/siggy" alt="crates.io"></a>
  <a href="https://johnsideserf.github.io/siggy/"><img src="https://img.shields.io/badge/docs-siggy-blue" alt="Docs"></a>
  <a href="https://ko-fi.com/johnsideserf"><img src="https://img.shields.io/badge/Ko--fi-Support%20siggy-ff5e5b?logo=ko-fi&logoColor=white" alt="Ko-fi"></a>
  <a href="https://x.com/siggyapp"><img src="https://img.shields.io/badge/follow-@siggyapp-000000?logo=x&logoColor=white" alt="Follow @siggyapp"></a>
</p>

<p align="center">
  <a href="../README.md">English</a>
  &nbsp;|&nbsp;
  <a href="README.da.md">Dansk</a>
  &nbsp;|&nbsp;
  <a href="README.de.md">Deutsch</a>
  &nbsp;|&nbsp;
  <a href="README.es.md">Español</a>
  &nbsp;|&nbsp;
  <a href="README.fr.md">Français</a>
  &nbsp;|&nbsp;
  <a href="README.it.md">Italiano</a>
  &nbsp;|&nbsp;
  <a href="README.nl.md">Nederlands</a>
  &nbsp;|&nbsp;
  <a href="README.pt-BR.md">Português</a>
  &nbsp;|&nbsp;
  <a href="README.fi.md">Suomi</a>
  &nbsp;|&nbsp;
  <a href="README.sv.md">Svenska</a>
  &nbsp;|&nbsp;
  <b>Русский</b>
  &nbsp;|&nbsp;
  <a href="README.uk.md">Українська</a>
  &nbsp;|&nbsp;
  <a href="README.zh-CN.md">简体中文</a>
  &nbsp;|&nbsp;
  <a href="../TRANSLATING.md">Предложить перевод</a>
</p>

Клиент мессенджера Signal для терминала в стиле IRC. Использует [signal-cli](https://github.com/AsamK/signal-cli) через JSON-RPC в качестве бэкенда обмена сообщениями.

![Скриншот siggy](../screenshot.png)

## Установка

### Homebrew (macOS)

```sh
brew tap johnsideserf/siggy
brew install siggy
```

### Готовые бинарные файлы

Скачайте последний релиз для своей платформы со страницы [Releases](https://github.com/johnsideserf/siggy/releases).

**Linux / macOS** (одной командой):

```sh
curl -fsSL https://raw.githubusercontent.com/johnsideserf/siggy/master/install.sh | bash
```

**Windows** (PowerShell):

```powershell
irm https://raw.githubusercontent.com/johnsideserf/siggy/master/install.ps1 | iex
```

Оба скрипта скачивают бинарный файл последнего релиза и проверяют наличие signal-cli.

### Из crates.io

Требуется Rust 1.70+.

```sh
cargo install siggy
```

### Сборка из исходного кода

Либо склонируйте репозиторий и соберите локально:

```sh
git clone https://github.com/johnsideserf/siggy.git
cd siggy
cargo build --release
# Бинарный файл находится в target/release/siggy
```

## Требования

- Установленный [signal-cli](https://github.com/AsamK/signal-cli), доступный в PATH (или указанный через `signal_cli_path`)
- Аккаунт Signal, привязанный как дополнительное устройство (мастер настройки поможет с этим)

## Использование

```sh
siggy                        # Запуск (использует файл конфигурации)
siggy -a +15551234567        # Указать аккаунт
siggy -c /path/to/config.toml  # Свой путь к файлу конфигурации
siggy --setup                # Повторно запустить мастер первоначальной настройки
siggy --demo                 # Запуск с тестовыми данными (signal-cli не нужен)
siggy --incognito            # Без локального хранения сообщений (только в памяти)
```

При первом запуске мастер настройки поможет найти signal-cli, ввести номер телефона и привязать устройство по QR-коду.

## Конфигурация

Конфигурация загружается из:
- **Linux/macOS:** `~/.config/siggy/config.toml`
- **Windows:** `%APPDATA%\siggy\config.toml`

```toml
account = "+15551234567"
signal_cli_path = "signal-cli"
download_dir = "/home/user/signal-downloads"
notify_direct = true
notify_group = true
desktop_notifications = false
inline_images = true
mouse_enabled = true
send_read_receipts = true
theme = "Default"
proxy = ""
```

Все поля необязательны. Значение `signal_cli_path` по умолчанию -- `"signal-cli"` (поиск через PATH), а `download_dir` по умолчанию -- `~/signal-downloads/`. В Windows укажите полный путь к `signal-cli.bat`, если его нет в PATH.

### Встроенные изображения внутри tmux

Вне tmux siggy автоматически распознаёт Kitty / iTerm2 / WezTerm / Ghostty и отображает вложения как нативные пиксельные изображения. Внутри tmux нужно настроить две вещи, потому что tmux скрывает внешний терминал от siggy:

1. Разрешите tmux пропускать неизвестные escape-последовательности. Требуется tmux 3.3+:

   ```
   set -g allow-passthrough on
   ```

   В более старых версиях tmux используется `set -g allow-passthrough all`.

2. Укажите siggy, какой протокол поддерживает внешний терминал (автоопределение видит только tmux):

   ```sh
   SIGGY_IMAGE_PROTOCOL=kitty siggy        # или iterm2 / sixel / halfblock
   ```

Если переменная `SIGGY_IMAGE_PROTOCOL` не задана, работает обычное автоопределение (вне tmux оно корректно, внутри откатывается на halfblock). Sixel проходит через tmux 3.4+ нативно, и переменная окружения для него не нужна.

## Возможности

- **Обмен сообщениями** -- Отправка и приём личных и групповых сообщений
- **Вложения** -- Предпросмотр изображений прямо в чате в виде halfblock-графики; вложения, не являющиеся изображениями, отображаются как `[attachment: имя_файла]`
- **Кликабельные ссылки** -- URL и пути к файлам оформлены как гиперссылки OSC 8 (кликабельны в терминалах вроде Windows Terminal, iTerm2 и других)
- **Индикаторы набора текста** -- Показывает, кто печатает, с подстановкой имени контакта
- **Синхронизация сообщений** -- Сообщения, отправленные с телефона, появляются в TUI
- **Постоянное хранение** -- Сообщения хранятся в SQLite с режимом WAL; беседы и отметки о прочтении сохраняются между запусками
- **Учёт непрочитанного** -- Счётчики непрочитанных в боковой панели и разделитель «новые сообщения» в чате
- **Уведомления** -- Звуковой сигнал терминала при новых сообщениях (настраивается отдельно для личных и групповых чатов, с отключением для отдельных бесед) и системные уведомления на рабочем столе
- **Имена контактов** -- Имена берутся из вашей адресной книги Signal; группы подгружаются автоматически при запуске
- **Реакции на сообщения** -- Реагируйте клавишей `r` в режиме Normal; выбор эмодзи и отображение счётчиков (`👍 2 ❤️ 1`)
- **Ответ / цитирование** -- Нажмите `q` на выделенном сообщении, чтобы ответить с цитатой
- **Редактирование сообщений** -- Нажмите `e`, чтобы отредактировать собственное отправленное сообщение
- **Удаление сообщений** -- Нажмите `d`, чтобы удалить сообщение локально или у всех (для своих сообщений)
- **Удаление бесед** -- Команда `/delete` удаляет текущую беседу локально (отклоняет ожидающие запросы на переписку)
- **Поиск по сообщениям** -- `/search <запрос>`, переход между результатами клавишами `n`/`N`
- **@упоминания** -- Введите `@` в групповом чате, чтобы упомянуть участника с автодополнением
- **Выделение сообщений** -- Подсветка текущего сообщения при прокрутке; `J`/`K` для перехода между сообщениями
- **Отчёты о прочтении** -- Значки статуса у исходящих сообщений (Отправляется → Отправлено → Доставлено → Прочитано → Просмотрено)
- **Исчезающие сообщения** -- Поддержка таймеров исчезающих сообщений Signal; задаются для каждой беседы командой `/disappearing`
- **Управление группами** -- Создание групп, добавление и удаление участников, переименование, выход из группы через `/group`
- **Запросы на переписку** -- Принимайте или удаляйте сообщения от незнакомых отправителей
- **Блокировка / разблокировка** -- Блокируйте контакты и группы командами `/block` и `/unblock`
- **Поддержка мыши** -- Клик по беседе в боковой панели, прокрутка сообщений, установка курсора кликом
- **Цветовые темы** -- Выбор темы через `/theme` или `/settings`
- **Мастер настройки** -- Первоначальная настройка при первом запуске с привязкой устройства по QR-коду
- **Vim-сочетания клавиш** -- Модальное редактирование (Normal/Insert) с полным управлением курсором
- **Автодополнение команд** -- Всплывающее окно с дополнением slash-команд по Tab
- **Окно настроек** -- Включайте и отключайте уведомления, боковую панель и встроенные изображения прямо из приложения
- **Адаптивный интерфейс** -- Боковая панель с изменяемой шириной, автоматически скрывается в узких терминалах (<60 столбцов)
- **Режим инкогнито** -- `--incognito` хранит всё только в памяти; после выхода ничего не остаётся
- **Поддержка прокси** -- Настройте TLS-прокси Signal через поле `proxy` в конфигурации для работы в сетях, где доступ к Signal ограничен или заблокирован
- **Демо-режим** -- Попробуйте интерфейс без signal-cli (`--demo`)

## Команды

| Команда | Алиас | Описание |
|---|---|---|
| `/join <name>` | `/j` | Перейти к беседе по имени контакта, номеру или названию группы |
| `/part` | `/p` | Покинуть текущую беседу |
| `/delete` | | Удалить текущую беседу (отклоняет ожидающие запросы на переписку) |
| `/attach` | `/a` | Открыть файловый менеджер, чтобы прикрепить файл |
| `/search <query>` | `/s` | Искать сообщения в текущей беседе (или во всех) |
| `/sidebar` | `/sb` | Показать/скрыть боковую панель |
| `/bell [type]` | `/notify` | Включить/выключить уведомления (`direct`, `group` или оба типа) |
| `/mute [duration]` | | Отключить/включить звук для текущей беседы (например, `1h`, `8h`, `1d`, `1w`) |
| `/block` | | Заблокировать текущий контакт или группу |
| `/unblock` | | Разблокировать текущий контакт или группу |
| `/disappearing <dur>` | `/dm` | Задать таймер исчезающих сообщений (`off`, `30s`, `5m`, `1h`, `1d`, `1w`) |
| `/group` | `/g` | Открыть меню управления группой |
| `/theme` | `/t` | Открыть выбор темы |
| `/contacts` | `/c` | Просмотреть синхронизированные контакты |
| `/settings` | | Открыть окно настроек |
| `/lock` | | Заблокировать сеанс |
| `/lock-reset` | | Сменить парольную фразу блокировки (нужна текущая фраза) |
| `/help` | `/h` | Показать справку |
| `/quit` | `/q` | Выйти из siggy |

Введите `/`, чтобы открыть окно автодополнения. Используйте `Tab` для дополнения и стрелки для навигации.

Чтобы написать новому контакту: `/join +15551234567` (формат E.164).

**Забыли парольную фразу блокировки?** Выйдите из siggy (или завершите процесс) и выполните `siggy --reset-lock`. Команда удалит сохранённый хеш парольной фразы и выведет путь к удалённому файлу. Следующий вызов `/lock` задаст новую парольную фразу.

## Горячие клавиши

Приложение использует модальное редактирование в стиле vim с двумя режимами: **Insert** (по умолчанию) и **Normal**.

### Глобальные (в обоих режимах)

| Клавиша | Действие |
|---|---|
| `Ctrl+C` | Выйти |
| `Tab` / `Shift+Tab` | Следующая / предыдущая беседа |
| `PgUp` / `PgDn` | Прокрутка сообщений (5 строк) |
| `Ctrl+Left` / `Ctrl+Right` | Изменить ширину боковой панели |

### Режим Normal

Нажмите `Esc`, чтобы перейти в режим Normal.

| Клавиша | Действие |
|---|---|
| `j` / `k` | Прокрутка вниз / вверх на одну строку |
| `J` / `K` | Перейти к предыдущему / следующему сообщению |
| `Ctrl+D` / `Ctrl+U` | Прокрутка вниз / вверх на полстраницы |
| `g` / `G` | Прокрутка в начало / в конец |
| `h` / `l` | Курсор влево / вправо |
| `w` / `b` | На слово вперёд / назад |
| `0` / `$` | В начало / конец строки |
| `x` | Удалить символ под курсором |
| `D` | Удалить от курсора до конца строки |
| `y` / `Y` | Скопировать текст сообщения / всю строку |
| `r` | Реакция на выделенное сообщение |
| `q` | Ответить на выделенное сообщение / процитировать его |
| `e` | Редактировать своё отправленное сообщение |
| `d` | Удалить сообщение (локально или у всех) |
| `n` / `N` | Следующее / предыдущее совпадение поиска |
| `i` | Перейти в режим Insert |
| `a` | Перейти в режим Insert (курсор на 1 вправо) |
| `I` / `A` | Перейти в режим Insert в начале / конце строки |
| `o` | Перейти в режим Insert (очистить поле ввода) |
| `/` | Перейти в режим Insert с уже введённым `/` |

### Режим Insert (по умолчанию)

| Клавиша | Действие |
|---|---|
| `Esc` | Перейти в режим Normal |
| `Enter` | Отправить сообщение / выполнить команду |
| `Shift+Enter` / `Alt+Enter` | Вставить перенос строки (для многострочных сообщений) |
| `Backspace` / `Delete` | Удалить символы |
| `Up` / `Down` | Перемещение по истории ввода |
| `Left` / `Right` | Переместить курсор |
| `Home` / `End` | В начало / конец строки |

## Архитектура

```
Keyboard --> InputAction --> App state --> SignalClient (mpsc) --> signal-cli (JSON-RPC stdin/stdout)
signal-cli --> JsonRpcResponse --> SignalEvent (mpsc) --> App state --> SQLite + Ratatui render
```

```
+------------+   mpsc channels   +----------------+
|  TUI       | <---------------> |  Signal        |
|  (main     |   SignalEvent     |  Backend       |
|  thread)   |   UserCommand     |  (tokio task)  |
+------------+                   +--------+-------+
                                          |
                                   stdin/stdout
                                          |
                                 +--------v-------+
                                 |  signal-cli    |
                                 |  (child proc)  |
                                 +----------------+
```

Создано на основе [Ratatui](https://ratatui.rs/) + [Crossterm](https://github.com/crossterm-rs/crossterm) с асинхронной средой выполнения [Tokio](https://tokio.rs/).

## Лицензия

[AGPL-3.0](../LICENSE)
