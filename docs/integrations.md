# Integraciones

Cómo Gentle Shell GPUI se conecta con Gentle Shell (el backend `gentle-shell --mode rpc` de cada pestaña de agente) y con los servicios externos: qué proceso lanza, qué protocolo habla, qué archivos lee o escribe y quién es su dueño. Al final, cómo agregar una integración nueva sin romper las fronteras de [`architecture.md`](architecture.md).

## Mapa

| Integración | Dónde vive | Transporte |
| --- | --- | --- |
| Backend de cada pestaña | `crates/pi-rpc/src/lib.rs` (`PiProcess`), `crates/app/src/features/session/` | Proceso hijo, JSONL por stdin/stdout |
| Resolución del lanzador y del PATH | `crates/pi-rpc/src/launch.rs` | Sistema de archivos, shell de login |
| Homes y carpeta de config | `crates/pi-rpc/src/home.rs` | Sistema de archivos |
| Inicio de sesión de proveedores | `crates/pi-rpc/src/login.rs`, `login-helper.mjs`, `features/providers/` | Proceso Node, JSONL |
| Subcomandos del lanzador (`upgrade`, `install`, `remove`, `setup`) | `features/settings/upgrade.rs`, `settings/packages.rs`, `setup/gentle.rs` | Proceso hijo |
| Engram (memoria) | `features/memory/client.rs`, `gentle_core::engram` | HTTP local |
| Uso de suscripciones | `features/usage/` | HTTPS |
| Actualización de Gentle Shell | `features/updates/` | HTTPS (npm registry) |
| Actualización de la app | `features/app_update/` | HTTPS (GitHub releases) |
| Instaladores del primer arranque | `features/setup/`, `features/onboarding/` | Procesos, HTTPS |
| Git | `features/git/` | Proceso `git` |
| Lienzo de Gentle Design | `features/browser/` | Web view del sistema |

Regla común: todo proceso hijo se crea con `pi_rpc::command` (nunca `std::process::Command::new` directo) y, si necesita encontrar herramientas del usuario, recibe `pi_rpc::child_path()` como `PATH`.

## Lanzador: `crates/pi-rpc/src/launch.rs`

### `resolve_backend(cwd) -> LaunchSpec`

Orden de búsqueda del programa:

1. `GENTLE_SHELL_BIN`, si no está vacío.
2. `bundled_launcher()`: la instalación autocontenida de Gentle Shell (4.7, la que crean su instalador y `gentle-shell upgrade`). Gana sobre un `gentle-pi` global de npm que quede en el PATH.
3. `gentle-shell` en el PATH combinado (`which::which_in` con `search_path()`).

Si nada aparece, el error pide `npm i -g gentle-pi` o `GENTLE_SHELL_BIN`.

`LaunchSpec { program, args, cwd, env }`:

| Campo | Valor |
| --- | --- |
| `args` | `GENTLE_SHELL_ARGS` partido por espacios (`extra_args()`), luego `--mode rpc` |
| `env` | `GENTLE_SHELL_INTERACTIVE_HOST=1` (activa los diálogos `ask_user_question` y la actividad en vivo de los ayudantes) y `PATH` = PATH combinado |
| `cwd` | La carpeta de la pestaña |

Los subcomandos del lanzador reutilizan el mismo `LaunchSpec`, pero toman sólo los argumentos anteriores a `--mode` (`spec.args.iter().take_while(|a| *a != "--mode")`) y agregan el suyo: `upgrade` (`settings/upgrade.rs`), `install`/`remove` (`settings/packages.rs`), `setup` (`setup/gentle.rs`, `launcher_command`).

### Instalación autocontenida

`bundled_candidates` define dónde se busca el lanzador:

| SO | Lanzador |
| --- | --- |
| macOS y Linux | `~/.gentle-shell/bin/gentle-shell` |
| Windows | `%LOCALAPPDATA%\gentle-shell\bin\gentle-shell.cmd`, luego `%USERPROFILE%\.gentle-shell-bundle\bin\gentle-shell.cmd` |

`bundled_install(launcher) -> Option<BundledInstall { node, package }>` reconoce el script generado por la marca `BUNDLED_LAUNCHER_MARK` (`"gentle-shell bundled launcher (generated; do not edit)"`) y deduce la disposición:

```text
<prefix>/
├── bin/gentle-shell[.cmd]        # script generado
├── runtime/node-<v>/bin/node     # o node.exe en Windows
├── current                       # enlace a la versión activa; en Windows, archivo con el nombre de la carpeta
├── versions/<v>/node_modules/gentle-pi
└── config/gentle-ai              # carpeta de config (ver Homes)
```

Lo usan `pi_package` (para encontrar `@earendil-works/pi-coding-agent`) y `LoginHelper::spawn` (para usar el Node propio de la instalación).

### PATH combinado

| Función | Qué hace |
| --- | --- |
| `login_shell_path()` (sólo Unix) | Ejecuta `$SHELL -l -i -c "printf '__GENTLE_PATH_START__%s__GENTLE_PATH_END__' \"$PATH\""` con 5 s de límite; las marcas aíslan el valor de lo que impriman los rc (nvm, fnm, mise). En Windows devuelve `None`. |
| `base_path()` | PATH del shell de login primero, luego el heredado sin duplicados. Se calcula una vez (`BASE_PATH`). |
| `set_extra_path(dirs)` | Carpetas que van antes que todo. `setup::apply_toolchain_path` pone aquí las toolchains que instaló la app (`<data dir>/gentle-shell-gpui/toolchain/{node,npm-global,bin}`). |
| `search_path()` / `child_path()` | Extra + base. Es el PATH que reciben todos los hijos. |
| `find_program(name)`, `resolve_node(cwd)` | Búsqueda en ese PATH. |
| `forget_cached()` | Olvida el PATH de login y los `--version` cacheados; llamarlo tras instalar algo. |

### Versión del backend

`version_output(spec)` ejecuta `<launcher> --version` y lo cachea por lanzador, invalidando cuando cambia el `Stamp` del archivo del lanzador o de `~/.gentle-shell/config.json`. `backend_version` extrae la línea `gentle-shell X.Y.Z`. `forget_versions()` se usa tras `gentle-shell upgrade`.

### `pi_rpc::command`

En Windows agrega `CREATE_NO_WINDOW` (`0x0800_0000`): sin él, cada hijo de consola (git, node, shells, el backend) abriría su propia ventana en una build release, y en debug se colgaría de la terminal que lanzó la app. En el resto de plataformas es `Command::new`.

## Homes: `crates/pi-rpc/src/home.rs`

`resolve_homes() -> Homes { agent, config }` replica `resolveHome` de gentle-pi (`lib/gentle-shell-launcher.ts`). Se cachea hasta que cambia el `Stamp` de `~/.gentle-shell/config.json`.

`agent` (el directorio de Pi: `auth.json`, `models.json`, `settings.json`, `sessions/`), en orden:

| Fuente | Resultado | ¿Aislado? |
| --- | --- | --- |
| `--link` en `GENTLE_SHELL_ARGS` | `user_pi_home()`: `GENTLE_SHELL_USER_PI_HOME`, `PI_CODING_AGENT_DIR` o `~/.pi/agent` | No |
| `--isolated` | `GENTLE_SHELL_HOME` o `~/.gentle-shell/agent` | Sí |
| `--home <dir>` | `<dir>` | No |
| `~/.gentle-shell/config.json` `mode`: `link` / `path` (`dir`) / `isolated` | Igual que arriba | Según modo |
| Nada | Aislado por defecto | Sí |

`config` (config de Gentle AI: `profiles.json`, `permission-mode.json`, …), en `config_home`:

1. `GENTLE_PI_CONFIG_HOME`, si no está vacío.
2. Si no hay `GENTLE_SHELL_BIN` y existe la instalación autocontenida: `<prefix>/config/gentle-ai`.
3. Si el home es aislado y es exactamente `~/.gentle-shell/agent`: `~/.gentle-shell/gentle-ai`.
4. En otro caso: `~/.pi/gentle-ai`.

`adopt_standalone_config()` se llama una vez en `main.rs` antes de abrir la ventana. La instalación 4.7 ya no lee `~/.gentle-shell/gentle-ai`; esta función copia a la carpeta nueva los archivos que le falten (recursivo, sin `.lock`, `.tmp` ni sobrescribir) y deja la marca `.adopted-standalone-config` para no repetirlo. La carpeta vieja no se toca.

Cualquier funcionalidad que lea configuración de Gentle Shell parte de `pi_rpc::resolve_homes()`; no se reconstruyen rutas a mano.

## Protocolo RPC

### Proceso y transporte (`crates/pi-rpc/src/lib.rs`, `record.rs`)

`PiProcess::spawn(&spec)` lanza el hijo con stdin/stdout/stderr en pipes y arranca dos hilos:

- `pi-rpc-stdout`: lee registros separados **sólo por LF** (`read_until(b'\n')`); los separadores Unicode (U+2028) son válidos dentro de cadenas JSON. `parse_record` quita el `\r` final y clasifica por `type`.
- `pi-rpc-stderr`: cada línea es `Incoming::Stderr`, nunca datos del protocolo.

| Registro de stdout | `Record` | Destino |
| --- | --- | --- |
| `type: "response"` con `id` pendiente | `Response` | El `request` que espera |
| `type: "response"` sin dueño y fallido | `Response` | `Incoming::Stderr("<command> failed: …")` |
| `type: "extension_ui_request"` | `UiRequest` | `Incoming::Ui` |
| Cualquier otro `type` | `Event(Value)` | `Incoming::Event` (se guarda como JSON: Pi agrega eventos con el tiempo) |
| Línea vacía | `Empty` | Se ignora |
| JSON inválido | — | `Incoming::Stderr("unparseable RPC record: …")` |

Al cerrarse stdout se vacían las peticiones pendientes (sus llamadores reciben error "gentle-shell exited before answering") y llega `Incoming::Exited(code)`.

`request(command)` asigna `id` = `gs-<n>`, escribe una línea y espera la respuesta; `success: false` se convierte en `Err(error)`. `respond_ui(id, UiAnswer)` responde a diálogos con `extension_ui_response` (`value`, `confirmed` o `cancelled: true`). `shutdown(grace)` cierra stdin y mata el hijo si sigue vivo tras `grace`; `Drop` llama a `shutdown(3 s)`.

### Ciclo de vida de una pestaña (`features/session/lifecycle.rs`)

- `start` limpia el estado de la pestaña, resuelve el `LaunchSpec` en el executor de fondo, agrega `--session <ruta>` si hay una conversación que reanudar (`backend.pending_session`), consulta la versión y llama a `PiProcess::spawn`. Luego `refresh_state`, el prompt pendiente (`-p`) y un bucle que agrupa los `Incoming` en cola (`drain_queued`) para aplicarlos con `handle_batch` en una sola actualización.
- Reanudar va en el arranque (`--session`) y no con `switch_session` después: Gentle Shell trata la sesión de arranque como nueva y la inicia en el modo de permisos guardado; una sesión cambiada empieza en Normal.
- `restart_when_idle` / `backend.restart_pending`: reinicios a mitad de turno (o con un diálogo abierto) esperan a que termine. Lo usan `reload` (`/reload`), `reload_models` (tras iniciar o cerrar sesión con un proveedor), `restart_for_update` (tras `gentle-shell upgrade`) y `choose_mode` (cambio de permisos).
- `retry_start` reintenta pestañas cuyo backend nunca arrancó (tras el onboarding).

### Comandos que envía la app

| `type` | Campos | Dónde | Uso |
| --- | --- | --- | --- |
| `prompt` | `message`, `images?`, `streamingBehavior: "steer"?` | `attachments::prompt_command`, `rpc.rs` `send_prompt_with` | Enviar; a mitad de turno se envía como steer |
| `prompt` | `message` (`/gentle:…`) | `rpc.rs` `run_extension_command` | Comando de extensión sin turno; sólo es éxito si `disposition == "handled"` |
| `clear_queue`, `abort` | — | `rpc.rs` `stop` | Detener el turno (⌘. / Ctrl+. o el botón Stop): vaciar la cola y luego abortar. Esc no detiene turnos; cierra paneles y diálogos |
| `bash` | `command`, `excludeFromContext` | `submit.rs` `run_shell` | `!cmd` / `!!cmd`; la salida llega por `bash_execution_update` |
| `get_state` | — | `refresh_state`, `set_model` | `model`, `thinkingLevel`, `sessionName`, `sessionFile`, `autoCompactionEnabled` |
| `get_available_models` | — | `refresh_state` | Selector de modelo |
| `get_available_thinking_levels` | — | `refresh_thinking_levels` | Deslizador de esfuerzo |
| `get_commands` | — | `refresh_state` | Menú `/` y paleta |
| `get_messages` | — | `refresh_state` | Reconstruir la conversación (`Transcript::load_messages`) |
| `get_session_stats` | — | `refresh_stats` | Costo y `contextUsage.percent` |
| `set_model` | `provider`, `modelId` | `set_model`, `workspace/overlays.rs` | Cambiar modelo |
| `set_thinking_level` | `level` | `view/effort.rs`, `workspace/overlays.rs` | Esfuerzo |
| `new_session` | — | `new_session` | ⌘N |
| `switch_session` | `sessionPath` | `open_session` | Abrir una sesión guardada en un backend vivo |
| `set_session_name` | `name` | `rename` | `/name`, doble clic en la pestaña |
| `compact` | `customInstructions?` | `compact` | `/compact` |
| `set_auto_compaction` | `enabled` | `set_auto_compaction` | Chip de contexto |
| `get_last_assistant_text` | — | `copy_last_answer` | `/copy` |
| `export_html` | `outputPath` | `export_to` | `/export` |

Las pantallas de ventana que necesitan otro comando usan `SessionView::send_command(Value)`, que informa errores en la conversación y refresca el estado.

### Eventos que maneja la app

`gentle_core::transcript::Transcript::apply` construye la conversación (`agent_start`, `agent_settled`, `message_start`, `message_update` con sus `assistantMessageEvent` `text_*`/`thinking_*`/`toolcall_start`, `message_end`, `tool_execution_start`/`_update`/`_end`, `compaction_start`/`_end`). Encima, `events.rs` `handle_event` reacciona a:

| Evento | Efecto |
| --- | --- |
| `queue_update` | Chip "N queued" (`steering` + `followUp`) |
| `compaction_start` / `compaction_end` | Estado de compactación; refresca estadísticas |
| `session_info_changed` | Nombre de la sesión |
| `thinking_level_changed` | Nivel de esfuerzo |
| `bash_execution_update` | Salida en vivo del `!cmd` activo |
| `tool_execution_start` | Fase ODD (`gentle_state::infer_phase`); etiqueta de jobs `bash_background` / `monitor` |
| `tool_execution_end` | Lista de tareas (`todo`), jobs (`details.gentleJobs`), lienzo de `gentle_design` → `SessionEvent::DesignCanvas` |
| `message_end` con `role: "custom"` | Actualiza jobs desde `gentle-jobs.notice` |
| Turno asentado (`effects.settled`) | Refresca estadísticas y Git; alerta `TurnEnded` salvo `aborted` |

Las tarjetas de mensajes personalizados (`gentle-agents.result`, `gentle-agents.message`, `gentle-jobs.notice`) se dibujan en `session/view/cards.rs`.

### Archivos de sesión

Los escribe Gentle Shell. La app sólo los lee:

- `features/sessions/discovery.rs`: `settings.json` `sessionDir` (con `~` expandido) o `<agent>/sessions/<session_dir_name(cwd)>/*.jsonl`; `summary.rs` hace un escaneo ligero con caché.
- `features/usage/stats.rs`: estadísticas locales de tokens y costo.
- La ruta de la sesión activa sale de `get_state.sessionFile`; la ventana la guarda en `workspaces.json` para reanudar con `--session`.

## Peticiones de UI de extensiones

`UiRequest { id, timeout, method: UiMethod }` (`crates/pi-rpc/src/record.rs`). Los métodos desconocidos se decodifican como `UiMethod::Unknown` y se ignoran. `UiMethod::is_dialog()` distingue los que bloquean a la extensión.

| Método | Mapeo en la app (`session/events.rs` `handle_ui`) |
| --- | --- |
| `select`, `confirm`, `input`, `editor` | Cola `extension.dialogs`; se dibuja el primero (`session/view/dialogs.rs`) y se responde con `answer_dialog` → `respond_ui`. Dispara la alerta `NeedsYou`. Si trae `timeout`, se quita de la cola al vencer (el agente ya lo resolvió). Esc responde `Cancelled`. |
| `notify` | `notifyType` → `NoticeLevel` (`error`, `warning`, el resto `Info`). Texto "Design canvas…" → `browser::canvas_notice` → lienzo. Avisos de límite de uso (`parse_rate_limit`, "`N% used (ventana)`") → chip en `extension.rate_limits`. El resto: captura del comando `/` en curso (`builtins::Capture`), toast sólo para warning/error (sin repetir, `first_toast`) y `SessionEvent::Notice` al centro de notificaciones (`features/notices/`). |
| `setStatus` | `extension.statuses[key] = text`; texto vacío o ausente borra la clave. |
| `setWidget` | `gentle-agents` → `gentle_core::activity::parse` + `retain_finished` (panel de agentes, chip Helpers). Otra clave → `extension.widgets` (un chip por widget con sus líneas). `widgetPlacement` se ignora. |
| `setTitle` | Título de la pestaña. |
| `set_editor_text` | Reemplaza el texto del compositor. |

Claves conocidas:

| Clave | Tipo | Tratamiento |
| --- | --- | --- |
| `gentle:mode` | status | Se parsea con `PermissionMode::from_status` ("Permissions: …") para el chip de permisos; oculto como texto. |
| `gentle:yolo` | status y widget | Oculto: el chip de permisos ya muestra el modo. |
| `gentle-jobs` | status | Oculto: los jobs tienen su propio chip, construido desde `details.gentleJobs`. |
| `gentle-agents` | widget | Modelo de actividad de ayudantes, no chip genérico. |
| Cualquier otra | status / widget | Texto genérico junto al compositor (`view/activity.rs`); en paneles estrechos sólo ícono con tooltip, en paneles mínimos se pliega en "+N". El código no da trato especial a ninguna clave de Engram. |

Los filtros están en `session/view/activity.rs` (`statuses` y `render_widget_chips`). `lifecycle.rs` `start` limpia estados, widgets y diálogos en cada arranque.

## Comandos `/` y pantallas nativas

Las pantallas de Gentle Shell son componentes de la TUI (`ctx.ui.custom()` no devuelve nada por RPC). `builtins::classify(text)` decide, en este orden:

1. `Submit::Builtin`: built-ins de Pi que la app ejecuta por RPC (`BUILTINS`: `compact`, `name`, `copy`, `export`, `reload`).
2. `Submit::Open(Screen)`: `gentle_core::commands::route` (`gentle:usage`, `gentle:stats`, `gentle:agents`, `gentle:jobs`, `gentle:profiles` sin argumentos, `gentle:commands`, `gentle:changes`). La pestaña emite `SessionEvent::OpenScreen` y `workspace/overlays.rs` `open_screen` abre la pantalla.
3. `Submit::Prompt`: se envía tal cual.

`gentle_core::commands::hidden` oculta del menú `/` y de la paleta los comandos sin efecto por RPC (`gentle:customize`, `gentle:guide`, `gentle:voice`, `gentle:music`, `gentle:panel`, `history`). La paleta aplica las mismas reglas en `palette/commands.rs` `command_row`.

Pantallas nativas que actúan sobre el backend con comandos de la propia shell: Profiles (`ProfilesEvent::RunCommand`, p. ej. `/gentle:profiles publish <name>`), Background subagents (`SettingsEvent::ShellCommand` → `run_extension_command("/gentle:background-subagents enable|disable")`).

## Archivos que lee y escribe la app

Rutas de la app: `dirs::data_dir()/gentle-shell-gpui/` (en macOS, `~/Library/Application Support/gentle-shell-gpui/`).

| Archivo | Dueño | Lee / escribe | Módulo |
| --- | --- | --- | --- |
| `<data>/gentle-shell-gpui/settings.json` (`theme`, `transparency`, `onboarding`) | App | L/E | `shared/themes.rs` (`store_path`, `write_key`), `features/setup/mod.rs` (`first_run`, `mark_done`) |
| `<data>/gentle-shell-gpui/workspaces.json` | App | L/E | `features/workspaces/store.rs` |
| `<data>/gentle-shell-gpui/app-update.json` | App | L/E | `features/app_update/cache.rs` |
| `<data>/gentle-shell-gpui/instance.json` (puerto y token, 0600) | App | L/E | `platform/instance.rs` |
| `<data>/gentle-shell-gpui/toolchain/` | App | E | `features/setup/` |
| `~/.gentle-shell/config.json` (`mode`, `dir`) | Lanzador | L | `pi-rpc/home.rs` |
| `~/.gentle-shell/config.json` (`startupReleaseCheck`, `skippedStartupRelease`) | Lanzador, compartido | L/E | `features/updates/` (`config_path` respeta `GENTLE_SHELL_CONFIG`) |
| `<agent>/auth.json` | Pi | L (la escritura la hace Pi vía `login-helper.mjs`) | `usage/credentials.rs`, `settings/snapshot.rs`, `setup/extras.rs` |
| `<agent>/settings.json` | Pi | L/E | `themes.rs` (`theme`), `sessions/discovery.rs` (`sessionDir`), `profiles/files.rs` (orquestador), `setup/gentle.rs` (`packages`) |
| `<agent>/models.json`, `<agent>/mcp.json` | Pi | L | `settings/snapshot.rs`, `setup/extras.rs` |
| `<agent>/subagents.json`, `<cwd>/.pi/subagents.json` | Gentle AI | L/E | `profiles/env.rs`, `profiles/apply.rs` |
| `<agent>/gentle-ai/managed-assets.json` | Instalador de Gentle AI | L/E | `profiles/apply.rs` |
| `<agent>/claude-bridge.json` | Claude bridge | L/E | `settings/claude.rs` |
| `<agent>/themes/*.json` | Usuario | L | `shared/themes.rs` |
| `<config>/profiles.json`, `<config>/models.json`, `<config>/profile-fallbacks.json` | Gentle AI | L/E | `profiles/files.rs`, `profiles/env.rs` |
| `<config>/permission-mode.json` (2 claves, 0600, escritura atómica) | Gentle AI | L/E | `shared/gentle_state.rs` |
| `<config>/notifications.json` | Gentle AI | L/E | `features/alerts/policy.rs` |
| `<config>/background-subagents.json`, `<cwd>/.pi/gentle-ai/background-subagents.json` | Gentle AI | L (cambios vía comando de la shell) | `settings/background.rs` |
| `<config>/banner.json` | Gentle AI | L | `shared/banner.rs`, `settings/snapshot.rs` |
| `<config>/persona.json`, `<cwd>/.pi/gentle-ai/persona.json` | Gentle AI | L | `settings/snapshot.rs` |
| `<cwd>/.pi/gentle-ai/profile.json`, `<git common>/gentle-ai/profile-pin.json` | Gentle AI | L/E | `profiles/pins.rs` |
| Sesiones `*.jsonl` | Pi | L | `sessions/`, `usage/stats.rs` |

`<agent>` y `<config>` son `pi_rpc::resolve_homes()`. Al escribir un archivo de Gentle Shell se respeta su esquema exacto (p. ej. `permission-mode.json` rechaza claves extra) y se escribe atómicamente (temporal + `rename`), como hacen `shared/gentle_state.rs` y `profiles/files.rs`.

## Otras integraciones

### Inicio de sesión de proveedores (`crates/pi-rpc/src/login.rs`)

Pi no tiene comando RPC de login. `LoginHelper::spawn(spec, homes)` escribe `login-helper.mjs` (incluido con `include_str!`) en el directorio temporal como `gentle-shell-gpui-login-<versión>.mjs` y lo ejecuta con Node: el de la instalación autocontenida (`bundled_install`) o `resolve_node`. Argumentos: el script y el paquete de Pi (`pi_package`); entorno: `PI_CODING_AGENT_DIR=<agent>` y el PATH combinado. El helper usa `ModelRuntime.login` de Pi y escribe `auth.json` él mismo.

Protocolo JSONL propio: la app envía `list`, `login` (`provider`, `authType: oauth|api_key`), `logout`, `response` (`id`, `value`), `cancel_prompt`, `cancel`; recibe `ready`, `providers`, `prompt` (`text`, `secret`, `select`, `manual_code`), `prompt_dismissed`, `event` (`info`, `auth_url`, `device_code`, `progress`), `done`, `error`. La UI está en `features/providers/`. Tras un cambio, la ventana llama a `reload_models` en cada pestaña: un backend sólo lee credenciales al arrancar.

### Engram (`features/memory/client.rs`)

HTTP GET bloqueante (ureq, 8 s, sin redirecciones) en el executor de fondo. URL base: `ENGRAM_URL`, si no `http://127.0.0.1:${ENGRAM_PORT}`, por defecto puerto `7437`. `ENGRAM_HTTP_TOKEN` agrega `Authorization: Bearer`. Rutas: `/project/current?cwd=`, `/observations/recent?…&limit=60`, `/search?q=…&limit=40` (con `project=` o `all_projects=true`). El modelo `Observation`, `encode` y `parse_observations` están en `gentle_core::engram`. La app no arranca Engram: lo hace Gentle Shell con la primera sesión.

### Uso de suscripciones (`features/usage/`)

`credentials.rs` lee `<agent>/auth.json` y resuelve claves como Pi (`$VAR`, `!comando`); `fetch.rs` hace un GET por tarjeta (20 s, sin redirecciones para que el token no salga del origen, `User-Agent: gentle-pi`): Codex en `https://chatgpt.com/backend-api/wham/usage` y NaN en `https://cloud-api.nan.builders/api/usage/quota`. Los parsers son `gentle_core::usage::subscription`.

### Actualización de Gentle Shell (`features/updates/`)

Igual que el chequeo de arranque del lanzador (`lib/gentle-shell-startup-update.ts`): `registry.rs` pide `https://registry.npmjs.org/gentle-pi/latest` (5 s, sólo versiones estables); `cache.rs` guarda `startupReleaseCheck` (`checkedAt`, `latest`, válido un día) y `skippedStartupRelease` en la config del lanzador, así la terminal y la app coinciden. La oferta se decide en `gentle_core::version::offer_for`. La instalación la hace `settings/upgrade.rs` con `gentle-shell upgrade`, luego `forget_cached` y `SettingsEvent::BackendUpdated` → `restart_for_update` en cada pestaña.

### Actualización de la app (`features/app_update/`)

`network.rs` consulta `https://api.github.com/repos/ismaelosuna7824/gentle-shell-gpui/releases/latest`; `release.rs` (puro) acepta sólo tags `vX.Y.Z` publicados, elige el asset con `asset_name` (`gentle-shell-gpui-<v>-macos-<arch>.zip`, `-windows-x86_64.zip`, `-linux-x86_64.tar.gz`) y verifica contra `SHA256SUMS`. `archive.rs` extrae, `install.rs` reemplaza el binario por SO y `relaunch.rs` reabre la app al salir. Caché de 6 h y versión saltada en `app-update.json`. Las versiones dev (`is_dev`) no se comprueban automáticamente.

### Instaladores (`features/setup/`)

`run_checks()` evalúa `CheckId` en el orden de `check_order` (Node, Go sólo en Windows, GentleShell, Setup, Provider, Git, ClaudeBridge, ClaudeLogin, Engram) con mínimos `MIN_NODE` 22.19.0, `MIN_GO` 1.25.10, `MIN_PI` 1.2.0, `MIN_ENGRAM` 3.0.0. Cada `Check` trae `Status` y `Fix` (`Auto`, `Terminal(cmd)`, `SignIn`, `None`). `install(id, log)`:

| Check | Instalación |
| --- | --- |
| Node | `fetch.rs` `install_node`: Node 22 LTS desde `https://nodejs.org/dist`, verificado por checksum, en `toolchain/node` |
| GentleShell | `npm install -g gentle-pi --allow-scripts=gentle-pi --foreground-scripts`; `install_prefix` agrega `--prefix` (el del Node gestionado, o `toolchain/npm-global` si el global requiere root) |
| Setup | `gentle-shell setup` |
| ClaudeBridge | `gentle-shell install npm:<bridge>` |
| Engram | Release de `Gentleman-Programming/engram` en GitHub, verificada, en `toolchain/bin` |
| Provider | Pantalla de Providers |

Lo que necesita `sudo` o login interactivo se entrega como `Fix::Terminal` y `features/onboarding/` lo abre en una pestaña de terminal. Tras instalar, `refresh_paths` (= `forget_cached` + `apply_toolchain_path`).

### Git (`features/git/`)

`git(cwd, args)` para lecturas (stderr descartado) y `run(cwd, args)` para efectos, con `GIT_TERMINAL_PROMPT=0` porque no hay terminal para credenciales. Los parsers están en `gentle_core::git`. La sesión refresca el snapshot al terminar el turno y, como máximo cada 1,5 s, cuando las herramientas tocan archivos (`refresh_git_soon`).

### Navegador para Gentle Design (`features/browser/`)

El lienzo se detecta por el resultado de `gentle_design` (`details.url`, `refreshed`, `stopped`) o por un `notify` "Design canvas: <url>" / "Design canvas refreshed: <url>" / "Design canvas stopped."; sólo se aceptan URLs de loopback (`http://127.0.0.1:` o `http://localhost:`). La ventana lo muestra en `BrowserPanel`. Implementación por SO: `macos.rs` (WKWebView como subvista), `windows.rs` (WebView2 en un popup propio que sigue a la ventana), `linux.rs` (WebKitGTK como hijo de la ventana X11; en Wayland el lienzo queda en el navegador que abrió Gentle Shell).

## Límites de un host RPC

- Gentle Shell no publica modelo, tokens ni costo de los ayudantes en vivo.
- Las ventanas de suscripción de Claude sólo llegan como cabeceras de respuesta dentro de la TUI; la app ve los avisos de límite (`notify`) pero no las ventanas completas.
- Ayudantes y jobs en segundo plano sólo los puede detener el agente ("Ask to stop" le pide ejecutar `subagent_cancel`).
- El modo Auto puede rechazar acciones: la entrada por RPC no cuenta como decisión humana en vivo.
- El modo de permisos de una sesión viva sólo cambia desde la TUI: la app guarda `permission-mode.json` y reinicia el backend sobre la misma conversación.
- Gentle Shell no reproduce sonidos para hosts RPC (`notificationContextAllowed`): `features/alerts/` los reproduce.
- Las pantallas `ctx.ui.custom()` no llegan: se reconstruyen de forma nativa o se ocultan.
- Las credenciales nuevas sólo se leen al arrancar el backend.

## Cómo agregar una integración

### Un chip de estado nuevo

Si la extensión ya publica `setStatus` o `setWidget`, aparece sola como chip genérico. Para darle trato propio:

1. `session/events.rs` `handle_ui`: interpretar la clave (como `gentle:mode` o `gentle-agents`). La interpretación pura va en `gentle_core` (como `PermissionMode::from_status` o `activity::parse`) con pruebas.
2. `session/state.rs`: el estado nuevo dentro del grupo correspondiente (`ExtensionUi` o `TurnActivity`).
3. `session/view/activity.rs`: excluir la clave del filtro genérico y dibujar el chip con `chip_popover` o `ui::chip`.
4. `session/lifecycle.rs` `start`: limpiar ese estado en cada arranque.

Si el dato viene de un evento o del resultado de una herramienta (como los jobs), se lee en `handle_event` / `TurnActivity` y no en `handle_ui`.

### Una pantalla nativa para un comando `/`

1. `crates/core/src/commands.rs`: variante en `Screen` y caso en `route`; quitarlo de `hidden` si estaba. Actualizar las pruebas (`routed_and_hidden_never_overlap`).
2. `workspace/overlays.rs` `open_screen`: abrir la pantalla para la variante nueva.
3. La pantalla, en su carpeta `features/<nombre>/`, con lectura de archivos vía `pi_rpc::resolve_homes()`.
4. Si debe pedir algo al backend, emitir un evento que la ventana convierta en `run_prompt` o `run_extension_command` (patrón de `ProfilesEvent::RunCommand` y `SettingsEvent::ShellCommand`), o usar `send_command` para un comando RPC.

La paleta y el compositor ya usan `route`/`hidden`; no hace falta tocarlos. Un built-in de Pi nuevo va en `session/builtins.rs` (`BUILTINS`, `Builtin`, `parse_builtin`) y `submit.rs` `run_builtin`.

### Un método de UI de extensión nuevo

1. `crates/pi-rpc/src/record.rs`: variante en `UiMethod` con los nombres de campo del protocolo (`#[serde(rename = …)]`); si bloquea, agregarla a `is_dialog`. Prueba en `ui_requests_map_wire_names`.
2. Si es diálogo: render y teclado en `session/view/dialogs.rs`, foco en `events.rs` `prepare_dialog`, respuesta con un `UiAnswer` existente (agregar uno en `lib.rs` sólo si el protocolo lo exige).
3. Si no bloquea: caso en `events.rs` `handle_ui`.

### Un comando RPC nuevo

1. Método en `session/rpc.rs` que llame a `self.request(json!({"type": …}), cx, |this, result, cx| …)` y use `report` para mostrar errores.
2. Si cambia el estado leído al arrancar, agregarlo a `refresh_state`.
3. Si el backend lo confirma con un evento, manejarlo en `handle_event` en lugar de suponer el resultado.

### Un archivo de configuración nuevo de Gentle Shell

1. Ruta a partir de `pi_rpc::resolve_homes()` (`agent` o `config`), nunca rutas fijas; considerar la variante de proyecto (`<cwd>/.pi/gentle-ai/`) si la shell la tiene.
2. Formato y validación en `gentle_core` cuando sean puros (como `notifications`); lectura, caché por `pi_rpc::file_cache::Stamp` y escritura atómica en la funcionalidad dueña.
3. Si la shell tiene un comando que cambia el archivo, preferirlo a escribirlo (patrón de `background.rs`).
4. Si el backend sólo lo lee al arrancar, reiniciar las pestañas con `restart_when_idle` o ofrecer `/reload`.
5. Agregarlo a la tabla de archivos de este documento.

### Un servicio HTTP nuevo

Seguir `memory/client.rs` y `usage/fetch.rs`: `ureq` con timeout global, `max_redirects(0)` si hay credenciales, `http_status_as_error(false)` y errores legibles; siempre en el executor de fondo; parsers y modelos en `gentle_core`.

## Checklist

- [ ] Procesos hijos con `pi_rpc::command`; `PATH` = `pi_rpc::child_path()` cuando busquen herramientas del usuario.
- [ ] Subcomandos del lanzador con los argumentos previos a `--mode` y el `env` del `LaunchSpec`.
- [ ] Rutas de Gentle Shell desde `resolve_homes()`; esquemas exactos y escrituras atómicas.
- [ ] Lógica pura (parsers, decisiones) en `gentle_core` con pruebas; IO en `app`.
- [ ] Nuevos `UiMethod` o claves con prueba de decodificación; estado limpiado en `start`.
- [ ] Reinicios del backend con `restart_when_idle`, nunca a mitad de turno.
- [ ] Errores de RPC visibles (`report`) y red tolerante a estar sin conexión.
- [ ] Tabla de archivos y límites de este documento actualizadas.
