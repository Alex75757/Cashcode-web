# cashcode-web

HTTP-сервис купюроприемника CCNET (CashCode / совместимые). Монопольно владеет COM-портом, опрашивает устройство, публикует статус и принимает команды по HTTP. Предназначен для работы с `laundry-stats` (или любым другим клиентом) по сети: `laundry-stats` **не открывает** COM/PTY, а вызывает `cashcode-web` по HTTP.

```
┌──────────────────┐  HTTP   ┌──────────────┐  COM/CCNET  ┌──────────────────┐
│  laundry-stats   │ ──────► │ cashcode-web │ ───────────► │  Bill validator  │
│  (cash_handler)  │ ◄────── │  :3000       │ ◄─────────── │  (COM4, CCNET)   │
└──────────────────┘  JSON   └──────────────┘              └──────────────────┘
```

## Что делает

- Открывает COM-порт (CCNET, 9600 или 19200 бод).
- `initialize()`: `RESET` → `poll` до `Idling` → `GET_BILL_TABLE` → `ENABLE`.
- Сразу после `initialize()` вызывает `disable()`, чтобы **до первого cash-заказа** купюроприемник не принимал купюры (индикатор красный).
- Опрашивает устройство (`poll`) каждые 200 мс, обрабатывает `EscrowPosition` / `BillStacked` / `BillReturned`.
- На `EscrowPosition` автоматически `stack()` (принять купюру).
- На `BillStacked`:
  - берёт номинал из **реальной таблицы устройства** (`dev.bill_table()`), не хардкод;
  - обновляет `CashState` (`total_rub`, `bills`);
  - ждёт `UnitDisabled`/`Idling` через `poll` (ACK продвигает state machine);
  - вызывает `enable_bill_types(BillMask::ALL)`, чтобы принимать следующую купюру для того же заказа.
- Принимает команды по HTTP: `enable`, `disable`, `return`, `reset`.
- Отдаёт `/api/status` с `ready`, `total_rub`, `bills`, `last_error`, `bill_table_loaded`.
- WebSocket `/ws` — события для отладочного UI (`BillEscrowed`, `BillStacked`, `BillReturned`, `Error`, `DeviceReady`).

## Требования

- Rust 1.90+ (для сборки), либо готовый `.exe` / бинарник для Linux.
- Купюроприемник CashCode (или совместимый) с CCNET, подключенный к COM-порту.
- Windows: драйвер USB-Serial (CH340/FTDI/CP210x), если используется USB-переходник. Порт виден в Диспетчере устройств как `COM4` (или другой).
- Linux: `/dev/ttyUSB0` или `/dev/ttyS0`, пользователь в группе `dialout`.

## Сборка

### Windows (`.exe`)

```powershell
cargo build --release --target x86_64-pc-windows-msvc
# готовый бинарник: target\release\cashcode-web.exe
```

Для статической линковки CRT (не требует VC++ Redistributable на целевой машине):

```powershell
$env:RUSTFLAGS = "-C target-feature=+crt-static"
cargo build --release --target x86_64-pc-windows-msvc
```

### Linux

```bash
sudo apt-get install -y libudev-dev pkg-config
cargo build --release
# готовый бинарник: target/release/cashcode-web
```

`libudev-dev` и `pkg-config` нужны для `serialport` (зависимость `cashcode`).

## Запуск

### Windows

```powershell
$env:CASHCODE_PORT = "COM4"
$env:RUST_LOG = "info,cashcode=info"
.\target\release\cashcode-web.exe
```

### Linux

```bash
CASHCODE_PORT=/dev/ttyUSB0 RUST_LOG="info,cashcode=info" ./target/release/cashcode-web
```

### Переменные окружения

| Переменная | По умолчанию | Описание |
|---|---|---|
| `CASHCODE_PORT` | `COM4` (Windows) | COM-порт купюроприемника (`COM4`, `/dev/ttyUSB0`, `/dev/ttyS0`) |
| `RUST_LOG` | — (см. ниже) | Фильтр `tracing`. Рекомендуется `info,cashcode=info` |

**`RUST_LOG`:** в коде фильтр задан как `EnvFilter::new("info,cashcode=info")` — глобально `INFO`, для `cashcode` (включая `cashcode::device`) `INFO`. `DEBUG cashcode::device` не идёт. Если нужен `DEBUG` для своего кода:

```powershell
$env:RUST_LOG = "info,cashcode_web=debug,cashcode=info"
```

## HTTP API

Базовый URL: `http://<host>:3000`.

### `GET /api/status`

```json
{
  "ready": true,
  "total_rub": 0.0,
  "bills": [],
  "last_error": null,
  "bill_table_loaded": true
}
```

- `ready` — устройство инициализировано и готово (`initialize()` прошёл).
- `total_rub` — сумма принятых купюр с последнего `/api/reset`.
- `bills` — список принятых купюр: `[{"bill_type": 2, "amount_rub": 100.0}]`.
- `last_error` — последняя ошибка (`open`, `initialize`, `poll`), если была.
- `bill_table_loaded` — загружена ли таблица номиналов с устройства.

### `POST /api/enable`

Включает приём купюр (`enable_bill_types(BillMask::ALL)`). Индикатор зелёный. Вызывается при создании cash-заказа.

```json
{"ok": true}
```

### `POST /api/disable`

Выключает приём. Индикатор красный. Вызывается после оплаты/отмены.

```json
{"ok": true}
```

### `POST /api/return`

Возвращает купюру из escrow (если есть). Вызывается при отмене cash-заказа.

```json
{"ok": true}
```

### `POST /api/reset`

Сбрасывает `total_rub = 0` и `bills = []`. Вызывается перед новым cash-заказом (и после оплаты/отмены).

```json
{"ok": true}
```

### `WS /ws`

WebSocket для отладочного UI. События:

- `{"type":"Welcome","data":{"message":"..."}}`
- `{"type":"DeviceReady"}`
- `{"type":"Prompt","data":{"message":"..."}}`
- `{"type":"BillEscrowed","data":{"bill_type":2}}`
- `{"type":"BillStacked","data":{"bill_type":2,"amount_rub":100.0}}`
- `{"type":"BillReturned","data":{"bill_type":2}}`
- `{"type":"Error","data":{"message":"..."}}`


## Номиналы купюр

Номиналы берутся из **реальной таблицы устройства** (`dev.bill_table()`), которую `cashcode` загружает при `initialize()` (`GET_BILL_TABLE`). Хардкода `1 => 50, 2 => 100` нет. Если `bill_table_loaded: false` в `/api/status` — таблица не загрузилась, `amount_rub` будет `0.0` (в логах `bill table not loaded`).

`bill_amount_from_table(table, bill_type)` использует `table.get(bill_type) -> Option<&BillEntry>` и `entry.denomination as f64`. Точное имя поля (`denomination`, `value`, `amount`) — в `docs.rs/cashcode/0.1.1/.../BillEntry`.

## Логи

`tracing` с фильтром `EnvFilter::new("info,cashcode=info")`:

- `INFO cashcode_web: Device opened on COM4` — порт открыт.
- `INFO cashcode_web: Device initialized successfully` — `initialize()` прошёл.
- `INFO cashcode_web: Device disabled after init (waiting for cash order)` — старт, приём выключен.
- `INFO cashcode_web: cashcode-web: enabled` — `POST /api/enable`.
- `INFO cashcode_web: Bill in escrow: type N` — купюра в escrow.
- `INFO cashcode_web: Bill stacked: type N` — купюра уложена.
- `INFO cashcode_web: Bill stacked, waiting for UnitDisabled before re-enable`.
- `INFO cashcode_web: Device ready for re-enable`.
- `INFO cashcode_web: Device re-enabled after stack (should be green)`.
- `WARN cashcode_web: timeout waiting for UnitDisabled after stack` — `poll` не дошёл до `UnitDisabled`/`Idling` (увеличьте таймаут или проверьте DIP/скорость).
- `ERROR cashcode_web: Poll error: ...` — ошибка связи, `cashcode-web` пытается `initialize()` заново.

## Структура проекта

```
cashcode-web/
├── Cargo.toml
├── src/
│   └── main.rs          # HTTP API + WS + run_validator_loop
├── cashcode-ui/         # опциональный отладочный UI (Vue)
│   └── dist/            # собранный SPA (ServeDir)
└── README.md
```

## Зависимости

```toml
[dependencies]
axum = "0.7"
cashcode = "0.1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
tower-http = { version = "0.5", features = ["fs"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

## Лицензия

Внутренний проект. Не распространяется отдельно от `pra4ka` без договора.
