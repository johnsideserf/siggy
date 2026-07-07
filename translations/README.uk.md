> This is the Ukrainian translation of the siggy README.
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
  <a href="README.ru.md">Русский</a>
  &nbsp;|&nbsp;
  <b>Українська</b>
  &nbsp;|&nbsp;
  <a href="README.zh-CN.md">简体中文</a>
  &nbsp;|&nbsp;
  <a href="../TRANSLATING.md">Запропонувати переклад</a>
</p>

Термінальний клієнт месенджера Signal в естетиці IRC. Працює поверх [signal-cli](https://github.com/AsamK/signal-cli) через JSON-RPC як бекенд обміну повідомленнями.

![знімок екрана siggy](../screenshot.png)

## Встановлення

### Homebrew (macOS)

```sh
brew tap johnsideserf/siggy
brew install siggy
```

### Готові бінарні файли

Завантажте останній реліз для своєї платформи зі сторінки [Releases](https://github.com/johnsideserf/siggy/releases).

**Linux / macOS** (одна команда):

```sh
curl -fsSL https://raw.githubusercontent.com/johnsideserf/siggy/master/install.sh | bash
```

**Windows** (PowerShell):

```powershell
irm https://raw.githubusercontent.com/johnsideserf/siggy/master/install.ps1 | iex
```

Обидва скрипти завантажують бінарний файл останнього релізу та перевіряють наявність signal-cli.

### З crates.io

Потрібен Rust 1.70+.

```sh
cargo install siggy
```

### Збірка з вихідного коду

Або клонуйте репозиторій і зберіть локально:

```sh
git clone https://github.com/johnsideserf/siggy.git
cd siggy
cargo build --release
# Бінарний файл буде в target/release/siggy
```

## Передумови

- Встановлений [signal-cli](https://github.com/AsamK/signal-cli), доступний через PATH (або вказаний через `signal_cli_path`)
- Обліковий запис Signal, пов'язаний як додатковий пристрій (майстер налаштування зробить це за вас)

## Використання

```sh
siggy                        # Запуск (використовує файл конфігурації)
siggy -a +15551234567        # Вказати обліковий запис
siggy -c /path/to/config.toml  # Власний шлях до конфігурації
siggy --setup                # Повторно запустити майстер першого налаштування
siggy --demo                 # Запуск із тестовими даними (signal-cli не потрібен)
siggy --incognito            # Без локального збереження повідомлень (лише в пам'яті)
```

Під час першого запуску майстер налаштування допоможе знайти signal-cli, ввести номер телефону та пов'язати пристрій через QR-код.

## Конфігурація

Конфігурація завантажується з:
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

Усі поля необов'язкові. Типове значення `signal_cli_path` -- `"signal-cli"` (пошук через PATH), а `download_dir` -- `~/signal-downloads/`. У Windows вкажіть повний шлях до `signal-cli.bat`, якщо його немає у PATH.

### Вбудовані зображення всередині tmux

Поза tmux siggy автоматично розпізнає Kitty / iTerm2 / WezTerm / Ghostty і відображає вкладення як нативні піксельні зображення. Усередині tmux потрібно налаштувати дві речі, оскільки tmux приховує зовнішній термінал від siggy:

1. Дозвольте tmux пропускати невідомі escape-послідовності. Потрібен tmux 3.3+:

   ```
   set -g allow-passthrough on
   ```

   У старіших версіях tmux використовуйте `set -g allow-passthrough all`.

2. Вкажіть siggy, яким протоколом володіє зовнішній термінал (автовизначення бачить лише tmux):

   ```sh
   SIGGY_IMAGE_PROTOCOL=kitty siggy        # або iterm2 / sixel / halfblock
   ```

Якщо `SIGGY_IMAGE_PROTOCOL` не задано, працює звичайне автовизначення (коректне поза tmux, усередині tmux відкочується до halfblock). Sixel проходить крізь tmux 3.4+ нативно й не потребує цієї змінної середовища.

## Можливості

- **Обмін повідомленнями** -- Надсилання та отримання особистих і групових повідомлень
- **Вкладення** -- Попередній перегляд зображень просто в чаті у вигляді halfblock-графіки; вкладення, що не є зображеннями, показуються як `[attachment: filename]`
- **Інтерактивні посилання** -- URL-адреси та шляхи до файлів є гіперпосиланнями OSC 8 (можна клацнути в терміналах на кшталт Windows Terminal, iTerm2 тощо)
- **Індикатори набору** -- Показує, хто саме друкує, з підставленням імені контакту
- **Синхронізація повідомлень** -- Повідомлення, надіслані з телефона, з'являються в TUI
- **Збереження даних** -- Зберігання повідомлень у SQLite з режимом WAL; розмови та позначки прочитаного переживають перезапуск
- **Облік непрочитаного** -- Лічильники непрочитаних повідомлень у бічній панелі та роздільник "нові повідомлення" в чаті
- **Сповіщення** -- Звуковий сигнал термінала про нові повідомлення (налаштовується окремо для особистих і групових, з вимкненням для окремих чатів) та системні сповіщення на робочому столі
- **Розпізнавання контактів** -- Імена з вашої адресної книги Signal; групи підвантажуються автоматично під час запуску
- **Реакції на повідомлення** -- Реагуйте клавішею `r` у режимі Normal; вибір емодзі з відображенням лічильників (`👍 2 ❤️ 1`)
- **Відповідь / цитування** -- Натисніть `q` на виділеному повідомленні, щоб відповісти з цитуванням контексту
- **Редагування повідомлень** -- Натисніть `e`, щоб відредагувати власні надіслані повідомлення
- **Видалення повідомлень** -- Натисніть `d`, щоб видалити локально або віддалено (для власних повідомлень)
- **Видалення розмов** -- Команда `/delete` видаляє поточну розмову локально (відхиляє очікувані запити на листування)
- **Пошук повідомлень** -- `/search <запит>` із `n`/`N` для переходу між результатами
- **@згадки** -- Введіть `@` у груповому чаті, щоб згадати учасників з автодоповненням
- **Вибір повідомлень** -- Підсвічування виділеного повідомлення під час прокручування; `J`/`K` для переходу між повідомленнями
- **Звіти про прочитання** -- Символи стану на вихідних повідомленнях (Надсилається → Надіслано → Доставлено → Прочитано → Переглянуто)
- **Повідомлення, що зникають** -- Дотримується таймерів зникнення повідомлень Signal; налаштовується для кожної розмови через `/disappearing`
- **Керування групами** -- Створення груп, додавання й видалення учасників, перейменування, вихід із групи через `/group`
- **Запити на листування** -- Приймайте або видаляйте повідомлення від невідомих відправників
- **Блокування / розблокування** -- Блокуйте контакти чи групи командами `/block` і `/unblock`
- **Підтримка миші** -- Клацайте розмови в бічній панелі, прокручуйте повідомлення, клацайте, щоб поставити курсор
- **Колірні теми** -- Вибір теми через `/theme` або `/settings`
- **Майстер налаштування** -- Покроковий помічник під час першого запуску з пов'язанням пристрою через QR-код
- **Клавіші у стилі Vim** -- Модальне редагування (Normal/Insert) з повноцінним переміщенням курсора
- **Автодоповнення команд** -- Спливаюче вікно з Tab-доповненням для slash-команд
- **Оверлей налаштувань** -- Перемикайте сповіщення, бічну панель та вбудовані зображення просто в застосунку
- **Адаптивний інтерфейс** -- Бічна панель зі змінною шириною, що автоматично ховається на вузьких терміналах (<60 колонок)
- **Режим інкогніто** -- `--incognito` зберігає все лише в пам'яті; після виходу нічого не лишається
- **Підтримка проксі** -- Налаштуйте TLS-проксі Signal через поле конфігурації `proxy` для роботи в мережах з обмеженнями
- **Демо-режим** -- Спробуйте інтерфейс без signal-cli (`--demo`)

## Команди

| Команда | Псевдонім | Опис |
|---|---|---|
| `/join <name>` | `/j` | Перейти до розмови за іменем контакту, номером або групою |
| `/part` | `/p` | Вийти з поточної розмови |
| `/delete` | | Видалити поточну розмову (відхиляє очікувані запити на листування) |
| `/attach` | `/a` | Відкрити файловий браузер, щоб прикріпити файл |
| `/search <query>` | `/s` | Шукати повідомлення в поточній розмові (або в усіх) |
| `/sidebar` | `/sb` | Показати або сховати бічну панель |
| `/bell [type]` | `/notify` | Увімкнути чи вимкнути сповіщення (`direct`, `group` або обидва) |
| `/mute [duration]` | | Заглушити поточну розмову або зняти заглушення (напр. `1h`, `8h`, `1d`, `1w`) |
| `/block` | | Заблокувати поточний контакт або групу |
| `/unblock` | | Розблокувати поточний контакт або групу |
| `/disappearing <dur>` | `/dm` | Встановити таймер зникнення повідомлень (`off`, `30s`, `5m`, `1h`, `1d`, `1w`) |
| `/group` | `/g` | Відкрити меню керування групою |
| `/theme` | `/t` | Відкрити вибір теми |
| `/contacts` | `/c` | Переглянути синхронізовані контакти |
| `/settings` | | Відкрити оверлей налаштувань |
| `/lock` | | Заблокувати сеанс |
| `/lock-reset` | | Змінити парольну фразу блокування (потрібна поточна парольна фраза) |
| `/help` | `/h` | Показати оверлей довідки |
| `/quit` | `/q` | Вийти із siggy |

Введіть `/`, щоб відкрити вікно автодоповнення. Натискайте `Tab` для доповнення та клавіші зі стрілками для навігації.

Щоб написати новому контакту: `/join +15551234567` (формат E.164).

**Забули парольну фразу блокування?** Вийдіть із siggy (або завершіть процес) і запустіть `siggy --reset-lock`. Команда видаляє збережений хеш парольної фрази та виводить шлях до видаленого файлу. Наступний виклик `/lock` встановить нову парольну фразу.

## Клавіатурні скорочення

Застосунок використовує модальне редагування у стилі vim із двома режимами: **Insert** (типовий) і **Normal**.

### Глобальні (обидва режими)

| Клавіша | Дія |
|---|---|
| `Ctrl+C` | Вийти |
| `Tab` / `Shift+Tab` | Наступна / попередня розмова |
| `PgUp` / `PgDn` | Прокрутити повідомлення (5 рядків) |
| `Ctrl+Left` / `Ctrl+Right` | Змінити розмір бічної панелі |

### Режим Normal

Натисніть `Esc`, щоб перейти в режим Normal.

| Клавіша | Дія |
|---|---|
| `j` / `k` | Прокрутити вниз / вгору на 1 рядок |
| `J` / `K` | Перейти до попереднього / наступного повідомлення |
| `Ctrl+D` / `Ctrl+U` | Прокрутити вниз / вгору на пів сторінки |
| `g` / `G` | Прокрутити на початок / у кінець |
| `h` / `l` | Перемістити курсор ліворуч / праворуч |
| `w` / `b` | На слово вперед / назад |
| `0` / `$` | На початок / у кінець рядка |
| `x` | Видалити символ під курсором |
| `D` | Видалити від курсора до кінця |
| `y` / `Y` | Скопіювати текст повідомлення / увесь рядок |
| `r` | Реагувати на виділене повідомлення |
| `q` | Відповісти на виділене повідомлення / процитувати його |
| `e` | Редагувати власне надіслане повідомлення |
| `d` | Видалити повідомлення (локально або віддалено) |
| `n` / `N` | Перейти до наступного / попереднього результату пошуку |
| `i` | Перейти в режим Insert |
| `a` | Перейти в режим Insert (курсор на 1 праворуч) |
| `I` / `A` | Перейти в режим Insert на початку / в кінці рядка |
| `o` | Перейти в режим Insert (очистити буфер) |
| `/` | Перейти в режим Insert з уже введеним `/` |

### Режим Insert (типовий)

| Клавіша | Дія |
|---|---|
| `Esc` | Перейти в режим Normal |
| `Enter` | Надіслати повідомлення / виконати команду |
| `Shift+Enter` / `Alt+Enter` | Вставити новий рядок (для багаторядкових повідомлень) |
| `Backspace` / `Delete` | Видалити символи |
| `Up` / `Down` | Гортати історію введення |
| `Left` / `Right` | Перемістити курсор |
| `Home` / `End` | Перейти на початок / у кінець рядка |

## Архітектура

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

Створено на основі [Ratatui](https://ratatui.rs/) + [Crossterm](https://github.com/crossterm-rs/crossterm) з асинхронним рантаймом [Tokio](https://tokio.rs/).

## Ліцензія

[AGPL-3.0](../LICENSE)
