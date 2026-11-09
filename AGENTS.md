# AGENTS.md

Contexto corto para quien trabaje en este repositorio, persona o agente de IA. El detalle está en [`docs/`](docs/README.md); aquí sólo lo que hay que saber antes de tocar código.

## Qué es

Aplicación de escritorio en Rust (GPUI + gpui-kit) para los agentes de Gentle Shell. Cada pestaña de agente es un proceso `gentle-shell --mode rpc` con el que la app habla por JSONL (`crates/pi-rpc`). Corre en macOS, Windows y Linux.

## Comandos

```bash
cargo run -p gentle-shell-gpui -- [CARPETA] [-p "texto"]   # -p "!cmd" corre un comando de shell
cargo fmt
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test
```

Antes de cada commit: `fmt`, `clippy` y `test` en verde. El CI corre `clippy` y `test` en los tres sistemas operativos.

## Dónde va cada cosa

| Necesitas… | Va en |
| --- | --- |
| Una pantalla o funcionalidad nueva | `crates/app/src/features/<nombre>/` (carpeta propia, entrada `mod.rs`) |
| Lógica pura: parsers, reglas, cálculos, formato | `crates/core/src/` (`gentle-core`), con sus tests |
| Hablar con el proceso de Gentle Shell, resolver su lanzador o sus homes | `crates/pi-rpc/src/` |
| Una pieza visual usada por varias pantallas | `crates/app/src/shared/` (sobre todo `ui.rs`) |
| Algo específico del sistema operativo | `crates/app/src/platform/` |

Reglas de dependencias y anatomía de una funcionalidad: [`docs/architecture.md`](docs/architecture.md).

## Reglas

- **Diseño:** usa los tokens y componentes de `crates/app/src/shared/ui.rs`; no inventes colores, bordes ni animaciones sueltas. El chrome de la ventana no lleva líneas: separa con espacio y cambios de superficie. Dentro de una pantalla superpuesta sólo existen las líneas finas de `ui::screen_header` y los pies. Ver [`docs/design-system.md`](docs/design-system.md).
- **Refactors sin cambio funcional** ([ADR 0003](docs/adr/0003-refactors-sin-cambio-funcional.md)): mover código no cambia lo que hace; un arreglo encontrado de paso va en su propio commit.
- **Sin abstracciones de una sola implementación:** no crees traits, adaptadores ni crates nuevos sin una segunda necesidad real.
- **Tests:** la lógica pura se prueba en `gentle-core`; los efectos de archivo, con directorios temporales. Nada de tests que sólo repiten la implementación.
- **Configuración de Gentle Shell:** la app lee y escribe archivos que también son de Gentle Shell. Antes de tocar uno, revisa quién es su dueño en [`docs/integrations.md`](docs/integrations.md).
- **Windows y Linux:** el código bajo `#[cfg(windows)]` o `#[cfg(target_os = "linux")]` no compila en macOS; cuida imports y rutas ahí, y confía en el CI para comprobarlo.

## Probar sin romper nada

- **No uses la app real de nadie para probar:** nunca le mandes clics ni teclas a una app que está usando una persona.
- **Abre una copia aislada (macOS y Linux):** `HOME=$(mktemp -d) target/release/gentle-shell-gpui` abre la app como primera vez (bienvenida incluida), sin tocar datos reales ni chocar con la instancia abierta. En Windows los datos van a `%APPDATA%`, así que un `HOME` temporal no aísla.
- **Andamios temporales:** si para llegar a un estado necesitas cambiar código (un temporizador, una variable de entorno), quítalo antes del commit y verifica con `grep` que no quedó.

Detalle, variantes de "máquina limpia" y capturas: [`docs/development.md`](docs/development.md).

## Publicar

`[workspace.package] version` en `Cargo.toml` → commit `release: X.Y.Z` → CI de `main` en verde → tag `vX.Y.Z` → el workflow `release.yml` compila y publica. Pasos completos en [`docs/development.md`](docs/development.md).
