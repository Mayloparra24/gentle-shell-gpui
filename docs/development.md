# Desarrollo

Cómo compilar, ejecutar, probar y publicar Gentle Shell GPUI, y cómo trabajar en el proyecto día a día sin tocar la app ni los datos reales de quien desarrolla. Las fronteras y reglas de código están en [`architecture.md`](architecture.md) y en los [ADR](adr/); aquí sólo se enlazan.

## Requisitos

| Requisito | Detalle |
| --- | --- |
| Rust | Estable, 1.88 o más nuevo (README). El workspace usa `edition = "2024"` y el código usa `if let … && …` (let chains). No hay `rust-toolchain` ni `rust-version`: CI instala `dtolnay/rust-toolchain@stable`. |
| Componentes | `clippy` (CI lo instala explícitamente) y `rustfmt`. |
| Gentle Shell | Para usar pestañas de agente: `npm install -g gentle-pi --allow-scripts=gentle-pi` y `gentle-shell setup`, o la instalación autocontenida en `~/.gentle-shell/bin/gentle-shell`. Sin él la app abre igual y muestra la pantalla de configuración. |
| Node.js | En el PATH (o el que instala la app en `<data dir>/gentle-shell-gpui/toolchain`). |
| Linux | Librerías de GPUI y WebKitGTK, las mismas que instala CI (`.github/workflows/ci.yml`). |
| macOS (empaquetar) | `codesign`, `ditto`, `hdiutil`, `plutil` (vienen con el sistema/Xcode CLT). Regenerar íconos: `python3`, `swift`, `iconutil`. |
| Windows (instalador) | Inno Setup 6 (`ISCC.exe`). |

Paquetes de Linux (Debian/Ubuntu), copiados de CI:

```bash
sudo apt-get install -y libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
  libvulkan-dev libx11-xcb-dev libxcb1-dev libfontconfig-dev libfreetype-dev \
  libwebkit2gtk-4.1-dev libgtk-3-dev
```

## Workspace

`Cargo.toml` raíz: `resolver = "2"`, miembros `crates/core`, `crates/pi-rpc`, `crates/app`. La versión, edición y licencia viven en `[workspace.package]` y los crates las heredan (`version.workspace = true`). Las dependencias compartidas (`anyhow`, `async-channel`, `regex`, `serde`, `serde_json`) se declaran en `[workspace.dependencies]`.

| Crate | Paquete | Qué es |
| --- | --- | --- |
| `crates/app` | `gentle-shell-gpui` (binario, `src/main.rs`) | La app GPUI. `build.rs` incrusta ícono y versión en el `.exe` de Windows; en otros SO no hace nada. |
| `crates/core` | `gentle-core` (lib `gentle_core`) | Lógica pura. Sólo depende de `regex`, `serde`, `serde_json`. |
| `crates/pi-rpc` | `pi-rpc` | Resolución del launcher, homes, cliente JSONL, login (`login-helper.mjs`). Sin GPUI. |

Perfiles (`Cargo.toml`):

| Perfil | Uso |
| --- | --- |
| `dev` | `debug = "line-tables-only"`; las dependencias (`[profile.dev.package."*"]`) con `opt-level = 2`, así un `cargo run` sin `--release` es usable. |
| `release` | Por defecto de Cargo: rápido de compilar, para probar localmente. |
| `dist` | Hereda `release` con `lto = "thin"`, `codegen-units = 1`, `strip = true`. Lo usan los scripts de `packaging/` y los workflows. Binario ~1/3 más chico, compilación mucho más lenta. |

## Compilar y ejecutar

```bash
cargo run                         # dev
cargo run --release -- .          # optimizado, abre la carpeta actual
cargo build --profile dist -p gentle-shell-gpui   # como en una release
```

### CLI

`crates/app/src/cli.rs`: `gentle-shell-gpui [FOLDER] [-p|--prompt TEXT]`.

| Argumento | Efecto |
| --- | --- |
| `FOLDER` | Selecciona (o crea) el workspace con esa raíz. Se ignora si no es un directorio. Sin carpeta se restaura el último workspace. |
| `-p TEXT`, `--prompt TEXT` | Abre una pestaña de agente nueva y, cuando el backend arranca, ejecuta el texto como si se hubiera escrito en el compositor (`SessionView::run_text` en `features/session/submit.rs`). |
| `-p "!cmd"` | Ejecuta `cmd` como comando de shell en esa pestaña; `-p "!!cmd"` lo mantiene fuera del contexto (convención de Pi). |

`-p` es útil en desarrollo para llevar la app a un estado concreto sin teclear en ella.

### Instancia única

`crates/app/src/platform/instance.rs`: una sola app por usuario, sea cual sea el build. La instancia primaria escucha en un puerto TCP de loopback y lo registra con un token aleatorio en `<data dir>/gentle-shell-gpui/instance.json` (modo 0600 en Unix). Un segundo lanzamiento envía su `OpenRequest { folder, prompt }` a la primaria y termina (`Role::Forwarded`); si no hay un listener vivo, se convierte en primaria.

Consecuencia práctica: con tu app instalada abierta, `cargo run` **no abre una ventana nueva**, sino que manda la carpeta y el prompt a la app que ya corre. Para correr un build de desarrollo al mismo tiempo, cambia el data dir (ver [Pruebas aisladas](#pruebas-aisladas)).

### Data dir

Todo lo de la app vive bajo `dirs::data_dir()/gentle-shell-gpui/`:

| Archivo | Dueño |
| --- | --- |
| `instance.json` | `platform/instance.rs` |
| `workspaces.json` | `features/workspaces/store.rs` |
| `settings.json` | `shared/themes.rs` (`store_path`): tema, transparencia y la clave `onboarding` (`{ "done": true, "version": … }`) que decide el primer arranque (`features/setup/mod.rs`: `first_run`, `mark_done`). |
| `app-update.json` | `features/app_update/cache.rs` |
| `toolchain/` | `features/setup/mod.rs` (Node y prefijo npm que instala la app) |

`dirs::data_dir()` es `~/Library/Application Support` en macOS, `$XDG_DATA_HOME` o `~/.local/share` en Linux y `%APPDATA%` en Windows. En macOS y Linux se deriva de `HOME`.

### Variables de entorno

| Variable | Dónde se lee | Efecto |
| --- | --- | --- |
| `GENTLE_SHELL_BIN` | `pi-rpc/src/launch.rs` (`resolve_backend`), `home.rs` | Launcher a usar. Orden normal: `GENTLE_SHELL_BIN`, instalación autocontenida (`bundled_launcher`: `~/.gentle-shell/bin/gentle-shell`; en Windows `%LOCALAPPDATA%\gentle-shell\bin\gentle-shell.cmd` o `%USERPROFILE%\.gentle-shell-bundle\bin\gentle-shell.cmd`), `gentle-shell` en el PATH combinado. Si está definida, el config home no se deriva del launcher autocontenido. |
| `GENTLE_SHELL_ARGS` | `launch.rs` (`extra_args`), `home.rs` | Opciones antes de `--mode rpc` (separadas por espacios): `--link`, `--isolated`, `--home <dir>`. También deciden qué home usa la app para `auth.json` y config. |
| `GENTLE_SHELL_HOME` | `home.rs` | Home aislado (por defecto `~/.gentle-shell/agent`). |
| `GENTLE_SHELL_USER_PI_HOME`, `PI_CODING_AGENT_DIR` | `home.rs` | Home de Pi para `--link` (por defecto `~/.pi/agent`). |
| `GENTLE_PI_CONFIG_HOME` | `home.rs` (`config_home`) | Gana sobre cualquier otra carpeta de config de Gentle Shell. |
| `GENTLE_SHELL_CONFIG` | `features/updates/mod.rs`, `features/setup/gentle.rs` | Ruta del `config.json` del launcher (por defecto `~/.gentle-shell/config.json`). |
| `GENTLE_PI_BACKGROUND_SUBAGENTS` | `features/settings/snapshot.rs` | Fuente de la política de background subagents mostrada en Configuración. |
| `GENTLE_SHELL_GENTLE_AI_PIN` | `features/setup/gentle.rs` | Pin de Gentle AI usado por la comprobación de setup. |
| `ENGRAM_URL`, `ENGRAM_PORT`, `ENGRAM_HTTP_TOKEN` | `features/memory/client.rs` | Servidor de Engram (puerto por defecto `7437`). |
| `ENGRAM_BIN` | `features/setup/extras.rs` | Binario de Engram para la comprobación. |
| `SHELL` | `launch.rs` (`login_shell_path`), `features/terminal/mod.rs` | Shell de login cuyo PATH se combina, y shell de los paneles de terminal. |
| `HOME` / `USERPROFILE` | `home.rs` (`home_dir`), `dirs` | Base de todos los homes y, en macOS/Linux, del data dir. |

La app siempre pasa al backend `GENTLE_SHELL_INTERACTIVE_HOST=1` y el `PATH` combinado (`LaunchSpec.env`). El resto del entorno (claves de proveedores, `GENTLE_SHELL_NO_AUTO_SETUP`, …) se hereda.

### PATH de los procesos hijos

Una app lanzada desde Finder o un `.desktop` hereda un PATH mínimo, sin nvm/fnm/mise. Por eso `pi-rpc` (`launch.rs`, `search_path`) arma un PATH propio:

1. Directorios extra que registra la app (`pi_rpc::set_extra_path`, llamado por `setup::apply_toolchain_path` al arrancar y antes de cada comprobación).
2. El PATH de un shell de login interactivo: `$SHELL -l -i -c "printf '__GENTLE_PATH_START__%s__GENTLE_PATH_END__' \"$PATH\""`, con 5 s de límite (sólo Unix).
3. El PATH heredado, sin duplicados.

Se cachea hasta `pi_rpc::forget_cached()`. Todo proceso hijo que necesite herramientas del usuario debe usar `pi_rpc::child_path()` y crearse con `pi_rpc::command` (en Windows añade `CREATE_NO_WINDOW`).

## Pruebas aisladas

Regla: **nunca** pruebes contra la app o los datos reales. No envíes teclas ni clics a una app que el usuario tenga abierta, ni edites su `workspaces.json`/`settings.json`.

### HOME temporal (primer arranque)

```bash
cargo build
HOME="$(mktemp -d)" ./target/debug/gentle-shell-gpui
```

Con un `HOME` vacío el data dir es otro, así que:

- `instance.json` es otro: el build se convierte en primario aunque tu app esté abierta.
- No hay `settings.json`: se abre la pantalla de bienvenida (`features/welcome/`), y sin `onboarding.done` vuelve a abrirse en cada arranque con ese mismo `HOME`.
- No hay `~/.gentle-shell` ni `auth.json`: Gentle Shell aparece sin configurar ni sesión iniciada.

En Linux, si `XDG_DATA_HOME` está definida, también hay que apuntarla al directorio temporal. En Windows `HOME` no cambia `%APPDATA%`; este método es para macOS y Linux.

### Máquina limpia (PATH mínimo)

`HOME` temporal no basta para simular una máquina sin Node ni Gentle Shell: `pi-rpc` combina el PATH del shell de login, que vuelve a traer nvm, Homebrew, etc. Para quitarlo, usa un `SHELL` falso que imprima un PATH mínimo con los marcadores que espera `login_shell_path`:

```bash
sandbox="$(mktemp -d)"
cat > "$sandbox/fake-shell" <<'EOF'
#!/bin/sh
printf '__GENTLE_PATH_START__%s__GENTLE_PATH_END__' "/usr/bin:/bin:/usr/sbin:/sbin"
EOF
chmod +x "$sandbox/fake-shell"
HOME="$sandbox" SHELL="$sandbox/fake-shell" PATH=/usr/bin:/bin:/usr/sbin:/sbin \
  ./target/debug/gentle-shell-gpui
```

El `PATH` heredado también se reduce porque se combina detrás del de login. Efectos secundarios: los paneles de terminal también usan `$SHELL` (`features/terminal/mod.rs`), así que en este modo no sirven; y las instalaciones de la configuración inicial escriben de verdad dentro de `$sandbox`.

Advertencia sobre el launcher autocontenido: `~/.gentle-shell/bin/gentle-shell` es un script generado con **rutas absolutas** al prefijo real (`<prefix>/runtime/node-<v>/bin/node`, `<prefix>/current/node_modules/gentle-pi/…`). Copiarlo al `HOME` temporal o apuntarle con `GENTLE_SHELL_BIN` ejecuta el Node y el paquete reales; `bundled_install` y `config_home` deducen el prefijo de la ubicación del launcher, así que una copia no encuentra `runtime/` y la detección queda a medias. Ese entorno no es una máquina limpia.

### Comprobaciones visuales

Toma capturas de la ventana del sandbox, no de la pantalla del usuario. En macOS:

```bash
screencapture -l <windowid> -o /tmp/shot.png
```

El id de ventana se obtiene de `CGWindowListCopyWindowInfo` (por ejemplo, un script `swift` que filtre por el PID del proceso del sandbox). Para llegar a un estado concreto usa `-p "texto"` / `-p "!cmd"` y la carpeta de la CLI en vez de simular entrada.

Si para alcanzar una pantalla hace falta código temporal (forzar un estado, un `eprintln!`, datos falsos), se elimina antes de hacer commit.

## Antes de hacer commit

```bash
cargo fmt
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

CI ejecuta clippy y las pruebas en macOS, Linux y Windows, pero **no** `cargo fmt --check`: el formato es responsabilidad local. `--locked` falla si `Cargo.lock` no está al día; commitea el lock junto con cualquier cambio de dependencias. Clippy corre con `-D warnings`, así que un `#[cfg(windows)]` con variables sin usar en macOS rompe CI en otra plataforma (el patrón existente es `#[cfg_attr(not(windows), allow(unused_mut))]`, `features/setup/mod.rs`).

[`architecture.md`](architecture.md) pide además una prueba de humo de la ruta modificada; hazla en un entorno aislado.

### Convenciones de pruebas

| Tipo | Dónde |
| --- | --- |
| Lógica pura | En `crates/core`, módulo `#[cfg(test)] mod tests` al final de cada archivo. Si una regla puede vivir en `core`, su prueba va allí, no en `app`. |
| Lógica de `app` | `#[cfg(test)] mod tests` en el mismo archivo (`features/app_update/release.rs`, `features/notices/store.rs`, …). |
| Efectos de archivo | Archivo `tests.rs` propio (`features/profiles/tests.rs`, `features/sessions/tests.rs`, `features/workspaces/tests.rs`) o ayudas compartidas (`features/app_update/mod.rs`: `mod test_support` con `temp_dir`, `release`, `fake_bundle`). |
| `pi-rpc` | `mod tests` en `launch.rs`, `home.rs`, etc.; algunos sólo Unix (`#[cfg(all(test, unix))]`). |

No hay pruebas de integración en `tests/` ni pruebas de UI GPUI. Las pruebas con archivos usan `std::env::temp_dir()` con un nombre único (prefijo `gs-…`, PID y a veces la hora) y lo borran al final; ver `platform/instance.rs` (`second_launch_is_forwarded_to_the_primary`). Las funciones con efectos reciben rutas como parámetro (por ejemplo `bundled_candidates(home, local_app_data, windows)`) para no depender del `HOME` real. Qué probar y qué no: [ADR-0003](adr/0003-refactors-sin-cambio-funcional.md).

## Convenciones de código

Están en [`architecture.md`](architecture.md); lo esencial:

- Una carpeta por funcionalidad en `crates/app/src/features/<nombre>/` con `mod.rs` como API; hijos con bloques `impl` y `pub(super)`.
- `shared/` sólo para piezas usadas por varias funcionalidades sin lógica de producto; `platform/` para integración con el SO.
- Lógica pura a `gentle-core` por comportamiento completo ([ADR-0002](adr/0002-nucleo-puro-incremental.md)); sin traits para una sola implementación.
- Refactors sin cambio funcional, separados de cambios de comportamiento ([ADR-0003](adr/0003-refactors-sin-cambio-funcional.md)); el historial los marca con `(no behavior change)`.
- Monolito modular ([ADR-0001](adr/0001-monolito-modular.md)).

## Commits y pull requests

Prefijos usados en `git log`:

| Prefijo | Uso |
| --- | --- |
| `feat(<área>):` | Comportamiento nuevo (`feat(welcome): …`). |
| `fix(<área>):` | Corrección (`fix(windows): …`, `fix(session): …`). |
| `refactor(<área>):` | Reorganización; termina en `(no behavior change)`. |
| `test(<área>):`, `docs:`, `perf:`, `ci:` | Menos frecuentes. |
| `release: X.Y.Z` | Sólo el cambio de versión (`Cargo.toml` + `Cargo.lock`). |

El área suele ser el nombre de la funcionalidad (`session`, `providers`, `app_update`) o de la plataforma. El asunto describe el efecto observable, en inglés y en minúscula tras los dos puntos.

Una PR por cambio, desde una rama `feat/…` o `refactor/…`. Se integran con merge commit (sin squash): la historia de la rama queda y el merge lleva el título de la PR con su número (`Welcome screen: … (#9)`).

## CI

| Workflow | Disparadores | Qué hace |
| --- | --- | --- |
| `ci.yml` (CI) | `push` a `main` y `pull_request`, ambos con `paths-ignore: ["**.md", "packaging/icons/**"]` | Matriz `macos-latest`, `ubuntu-latest`, `windows-latest` (`fail-fast: false`): `cargo clippy --workspace --all-targets --locked -- -D warnings` y `cargo test --workspace --locked`. Caché `Swatinem/rust-cache@v2`. |
| `cache-warm.yml` (Cache warm) | `push` a `main` que toque `Cargo.lock`, `**/Cargo.toml`, `cache-warm.yml` o `release.yml`; `workflow_dispatch` | Compila `--profile dist` para los cuatro targets y guarda la caché con `shared-key: dist-<target>`. Existe porque un tag sólo puede restaurar cachés de la rama por defecto. |
| `release.yml` (Release) | Tags `v*`; `workflow_dispatch` (dry run) | Ver [Releases](#releases). |

Concurrencia: CI usa `group: ci-${{ github.ref }}` con `cancel-in-progress: true` (un push nuevo a la misma rama cancela el anterior); Cache warm cancela también; Release usa `cancel-in-progress: false`.

Un cambio que sólo toca Markdown o `packaging/icons/` no ejecuta CI.

## Releases

Assets que produce `release.yml` (los nombres los consume el actualizador):

| Plataforma | Runner / target | Assets |
| --- | --- | --- |
| macOS Apple silicon | `macos-latest`, `aarch64-apple-darwin` | `gentle-shell-gpui-<v>-macos-aarch64.dmg`, `.zip` |
| macOS Intel | `macos-latest` (compilación cruzada), `x86_64-apple-darwin` | `gentle-shell-gpui-<v>-macos-x86_64.dmg`, `.zip` |
| Windows | `windows-latest`, `x86_64-pc-windows-msvc` | `gentle-shell-gpui-<v>-windows-x86_64.zip` (exe), `…-windows-x86_64-setup.exe` |
| Linux | `ubuntu-22.04`, `x86_64-unknown-linux-gnu` | `gentle-shell-gpui-<v>-linux-x86_64.tar.gz` |
| Todas | `ubuntu-latest` | `SHA256SUMS` (`sha256sum` de los siete assets) |

Pasos del workflow:

1. `version`: lee `[workspace.package] version` de `Cargo.toml`. En un tag, falla si `GITHUB_REF_NAME != v<version>`; en `workflow_dispatch`, el input `version` opcional debe coincidir.
2. `build`: restaura la caché `dist-<target>` (`save-if: false`), instala librerías en Linux y empaqueta: `packaging/macos/bundle.sh <target> <v>` (con `MACOSX_DEPLOYMENT_TARGET=11.0`, firma ad hoc `codesign --sign -`, zip con `ditto` sin `._*`, DMG con acceso directo a `/Applications`), `packaging/linux/package.sh <target> <v>` (binario, `.desktop`, `icon-512.png`), y en Windows `cargo build --profile dist`, `Compress-Archive` y `ISCC` sobre `packaging/windows/installer.iss` (instala Inno Setup con `choco` si falta).
3. `release`: comprueba que estén los siete assets, escribe `SHA256SUMS` y, sólo en `push`, publica con `gh release create --verify-tag --generate-notes`, título `Gentle Shell <v>`. Una versión con `-` (`0.2.0-rc.1`) sale como `--prerelease`. En `workflow_dispatch` sube todo como artefacto `release-dry-run-<v>` sin publicar.

### Cortar una release

1. En `main` actualizado, cambia `version` en `[workspace.package]` del `Cargo.toml` raíz y ejecuta `cargo check` para que `Cargo.lock` se actualice.
2. Commit sólo con esos dos archivos: `release: X.Y.Z`. Push a `main`.
3. Espera a que CI de `main` pase (y Cache warm, que se dispara porque cambió `Cargo.lock`, para que el tag reutilice la caché).
4. Opcional: dry run desde Actions → Release → Run workflow.
5. `git tag vX.Y.Z && git push origin vX.Y.Z`.
6. Revisa que la release tenga los ocho archivos.

### Cómo los consume el actualizador

`crates/app/src/features/app_update/`:

| Módulo | Papel |
| --- | --- |
| `network.rs` | `GET https://api.github.com/repos/ismaelosuna7824/gentle-shell-gpui/releases/latest`, descarga con SHA-256 y progreso. |
| `release.rs` | Sólo cuenta releases publicadas, no `draft`, no `prerelease`, con `vX.Y.Z` estable. `asset_name(version, os, arch)` elige `macos-<arch>.zip`, `windows-x86_64.zip` o `linux-x86_64.tar.gz` (no hay asset para Linux/Windows aarch64). `expected_digest` lee `SHA256SUMS`. `is_dev`: `0.0.0` o versión no estable no comprueba al arrancar. |
| `cache.rs` | `app-update.json`: caché de 6 h (`TTL_MS`) y versión saltada. |
| `archive.rs` | Extracción segura de zip y tar.gz. |
| `install.rs` | `install_target`: en macOS reemplaza el `.app` que contiene al ejecutable (sin bundle, o en `AppTranslocation`, no instala); en Linux el ejecutable (conserva permisos); en Windows el exe en uso. En Windows y Linux rechaza ejecutables bajo `target/[<triple>/]{debug,release,dist}/` (`in_cargo_target`). |
| `relaunch.rs` | Proceso que reabre la app al salir; `remove_leftovers` borra al arrancar el `gentle-shell-gpui.exe.old` que deja una actualización en Windows. |

Por eso: renombrar un asset, cambiar el formato de `SHA256SUMS` o el nombre del binario (`BINARY = "gentle-shell-gpui"`) rompe las actualizaciones de las versiones ya instaladas. Las prereleases nunca se ofrecen dentro de la app. El DMG y el `setup.exe` no los usa el actualizador.

## Notas por plataforma

### macOS

- `packaging/macos/Info.plist` (identificador `io.github.ismaelosuna7824.gentle-shell-gpui`, `@VERSION@` y `@MIN_MACOS@` sustituidos por `bundle.sh`). `GENTLE_BIN=<ruta>` empaqueta un binario ya compilado.
- Fuera de un `.app` (`cargo run`), `platform/icons.rs` (`set_dock_icon`) pone el ícono del Dock en tiempo de ejecución con `icon-512.png`; dentro del bundle usa `AppIcon.icns`.
- Firma ad hoc, sin notarizar: el primer arranque lo bloquea Gatekeeper (README). `bundle.sh` limpia xattrs antes de firmar porque romperían el sello.

### Windows

- `main.rs`: `#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]`. Un build release no tiene consola (no hay `stderr` visible); un build debug sí.
- Todo proceso hijo pasa por `pi_rpc::command` (`CREATE_NO_WINDOW`) para no abrir ventanas de consola.
- `crates/app/build.rs` incrusta `packaging/icons/icon.ico` (recurso 1, el que GPUI usa para la ventana) y la versión del paquete con `embed-resource`.
- `installer.iss`: por usuario (`PrivilegesRequired=lowest`), en `{localappdata}\Programs\Gentle Shell`, `CloseApplications=yes`, `RestartApplications=yes`. No cambies `AppId`: Windows identifica las actualizaciones y el desinstalador por él.

### Linux

- El ícono de ventana sólo lo aplica GPUI en X11: `platform/icons.rs` (`window_icon`) lo reduce a 128 px por los límites de `_NET_WM_ICON`. `app_id` es `dev.gentle-shell.gpui`, igual que `StartupWMClass` en `packaging/linux/gentle-shell-gpui.desktop`.
- El navegador integrado usa WebKitGTK como hijo de la ventana X11 (`wry` con `x11`, `gtk`); Wayland no tiene ventanas hijas.
- En ejecución hace falta `libwebkit2gtk-4.1-0`.

### Íconos

Fuente: el arte `ROSE` de `crates/app/src/shared/banner.rs`. Tras editarlo, en macOS:

```bash
packaging/icons/generate.sh
```

Ejecuta `generate.py` (SVG), `render.swift` (PNG en varios tamaños), `iconutil` (`AppIcon.icns`) e `ico.py` (`icon.ico`), y escribe `icon-1024.png`, `icon-512.png`. CI nunca lo ejecuta: commitea los resultados. `icon-512.png` se incrusta en el binario con `include_bytes!`, así que un cambio de ícono requiere recompilar.

## Solución de problemas

| Síntoma | Causa y salida |
| --- | --- |
| `cargo run` no abre ventana y termina | Otra instancia (tu app instalada) recibió la petición. Usa un `HOME` temporal. |
| `gentle-shell was not found on your PATH` | `resolve_backend` no encontró launcher. Instala `gentle-pi`, o define `GENTLE_SHELL_BIN`. |
| Desde Finder/`.desktop` no se encuentra Node | La app hereda un PATH mínimo. Comprueba que tu shell de login (`$SHELL -l -i`) exporte el PATH de nvm/fnm/mise y responda en menos de 5 s; si imprime mucho, no importa (se recorta entre marcadores). Tras instalar algo, `pi_rpc::forget_cached()` recalcula el PATH. |
| Perfiles, rutas de modelos o permisos dejaron de aplicarse tras actualizar Gentle Shell a 4.7 | La instalación autocontenida lee su config de `<prefix>/config/gentle-ai`, no de `~/.gentle-shell/gentle-ai`. `main.rs` llama a `pi_rpc::adopt_standalone_config()` al arrancar: copia una vez lo que falta (no sobrescribe; deja el marcador `.adopted-standalone-config`) e informa por `stderr`. |
| Un global `gentle-pi` del PATH no se usa | Gana la instalación autocontenida (`bundled_launcher`) salvo que definas `GENTLE_SHELL_BIN`. |
| La bienvenida aparece en cada arranque | Falta `onboarding.done` en `settings.json` del data dir (normal con `HOME` temporal). |
| La app no ofrece o no instala una actualización | No la ofrece: versión de desarrollo (`is_dev`), caché de 6 h en `app-update.json`, versión saltada, o la última release es prerelease. No la instala: binario fuera de un `.app` (macOS) o bajo `target/` (`install_target`). |
| Iniciar o cerrar sesión de un proveedor reinicia las pestañas | El backend sólo lee credenciales al arrancar; es intencional (README, Providers). |
| CI falla sólo en Linux | Faltan librerías del sistema o código bajo `cfg(target_os = "linux")` que no se compila en macOS; revisa los avisos de clippy de esa plataforma. |
| `--locked` falla en CI | `Cargo.lock` no se commiteó tras cambiar dependencias o la versión. |

## Checklist

### Pull request

- [ ] Una funcionalidad o corrección por PR; refactors aparte y marcados `(no behavior change)`.
- [ ] Código en la carpeta de su funcionalidad; lógica pura en `gentle-core` con pruebas.
- [ ] Pruebas para reglas, límites o errores nuevos; las de archivos usan directorios temporales únicos.
- [ ] `cargo fmt`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked`.
- [ ] Prueba de humo en un `HOME` temporal; capturas sólo de la ventana del sandbox.
- [ ] Sin código temporal para alcanzar estados.
- [ ] `Cargo.lock` incluido si cambió.
- [ ] `architecture.md` o un ADR actualizado si cambia una frontera; README si cambia algo visible.
- [ ] Commits con prefijo `feat|fix|refactor|test|docs(<área>):`.

### Release

- [ ] `main` verde en CI.
- [ ] `version` cambiada en `[workspace.package]`, `cargo check` ejecutado, commit `release: X.Y.Z` con `Cargo.toml` y `Cargo.lock`.
- [ ] CI de ese commit verde (y Cache warm terminado).
- [ ] Opcional: dry run de Release.
- [ ] Tag `vX.Y.Z` igual a la versión del workspace, empujado.
- [ ] Release publicada con 7 assets y `SHA256SUMS`; nombres sin cambiar.
- [ ] Si es estable, la versión anterior instalada ofrece la actualización (Configuración → App → Check for updates).
