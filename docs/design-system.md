# Sistema de diseño

Cómo se ve la app y con qué piezas se construye. Todo vive en `crates/app/src/shared/`: `ui.rs` (tokens, componentes y movimiento), `themes.rs` (catálogo y tema activo), `banner.rs` y `surge.rs` (marca y efectos pixel art) y `assets.rs` (íconos). Las vistas usan estas piezas en lugar de colores y medidas sueltas, para que cada pantalla se parezca a las demás.

## Principios

Estos principios salen del código actual, no son aspiraciones.

| Principio | Cómo se aplica |
| --- | --- |
| **Vidrio translúcido** | La ventana pide `WindowBackgroundAppearance::Blurred` y una barra de título transparente (`crates/app/src/app/mod.rs`). Las superficies (`panel`, `surface`, `well`…) son el fondo del tema teñido y con alfa, para que se vea el escritorio desenfocado. Lo que flota encima (menús, pantallas, paleta) es opaco: `popover_bg` y `screen_bg` tienen alfa 1 con cualquier transparencia. |
| **Sin líneas en el chrome** | La barra de título no tiene borde (`title_bar_border = clear` en `ui::apply_theme`). Paneles y barra lateral llevan `border_1()` transparente; sólo el panel enfocado de una división muestra un borde `petal().opacity(0.45)` (`features/workspace/view.rs`). Los divisores entre paneles son un hueco de 6 px (`DIVIDER`) con una barrita de 2×40 px que aparece en `petal()` sólo al pasar el cursor. Dentro, las zonas se separan con espacio y con el cambio de superficie (`panel` → `surface` → `well`). |
| **Líneas finas sólo en pantallas** | Dentro de una pantalla superpuesta sí hay una línea `ui::border()` entre cabecera, cuerpo y pie (`screen_header` usa `border_b_1`, los pies `border_t_1`), y las filas de Configuración usan `border().opacity(0.5)`. No se agregan más. |
| **Petal como acento** | El rosa de marca (el `accent` del tema) se reserva para lo vivo y lo principal: el punto que late mientras el agente trabaja, el texto `✿ fase…`, el panel enfocado, los botones primarios y el resplandor del welcome. No se usa como relleno de fondo. |
| **Denso pero tranquilo** | `text_xs` es el tamaño más usado y `muted` el color más usado (más que `fg`). El color semántico va en íconos, chips y bordes de tarjetas, no en bloques grandes. Las animaciones duran 220–420 ms y no rebotan más de un ~3 %. |

## Color

### De dónde salen los colores

```text
themes/*.json (formato de tema de Pi)
  → themes::parse_pi_theme   → Theme (colores opacos 0xRRGGBB)
  → ui::Palette::new(theme, clear)   → roles con alfa, según la transparencia
  → ACTIVE (RwLock global en themes.rs)
  → ui::fg(), ui::surface()…  (leen con themes::palette(|p| …), sin cx)
  → ui::apply_theme(cx)       → colores de gpui-component (botones, inputs, listas, menús)
```

- **Catálogo** (`themes::load_catalog`): primero los temas de Gentle Shell embebidos con `include_str!` desde `crates/app/themes/` (`Gentleman-Cute`, `Gentleman-Sexy`, `Gentle`); los instalados en `<gentle-pi>/themes/` los reemplazan por nombre. Después los temas del usuario en `<agent dir>/themes/` y al final los esquemas populares de `SCHEMES`. Los nombres son únicos; gana el primero.
- **Tema inicial** (`themes::pick_default`): el guardado en `dirs::data_dir()/gentle-shell-gpui/settings.json` (clave `theme`), si no el `theme` del `settings.json` de Pi, si no `DEFAULT_THEME` (`"Gentleman-Cute"`).
- **Cambios en caliente**: `themes::select(index, cx)` y `themes::set_transparency(clear, persist, cx)` reinstalan la paleta, llaman a `ui::apply_theme` y refrescan todas las ventanas. Como los tokens se leen en cada render, ninguna vista guarda colores.

Campos del JSON que usa `parse_pi_theme` (el resto del formato de Pi se ignora salvo para la tabla ANSI de la terminal):

| `Theme` | JSON | Si falta |
| --- | --- | --- |
| `bg` | `export.pageBg`, si no `vars.bg` | `0xffffff` claro / `0x0e0e11` oscuro |
| `fg`, `muted`, `faint` | `colors.text`, `colors.muted`, `colors.dim` | mezclas de `fg` y `bg` |
| `accent`, `accent_active` | `colors.accent`, `colors.borderAccent` | `fg`; mezcla de `accent` y `fg` |
| `raised` | `colors.userMessageBg` | `mix(bg, accent, 0.14)` |
| `border` | `colors.border` | `mix(bg, fg, 0.25)` |
| `heading` | `colors.toolTitle`, si no `colors.mdHeading` | `warning` |
| `info` | `vars.blue`, si no `colors.syntaxFunction` | ANSI azul |
| `success`, `danger`, `warning` | `colors.success`, `colors.error`, `colors.warning` | valores fijos |
| `selection` | `colors.selectedBg` | `mix(bg, accent, 0.25)` |

`light` se decide por la luminancia de `bg` (> 0.5), o la de `text` si no hay fondo.

### Tokens

Todas son funciones sin argumentos en `crate::shared::ui` que devuelven `Hsla`.

| Token | Sale de | Para qué |
| --- | --- | --- |
| `fg()` | `fg` | Texto principal, títulos, valores. |
| `muted()` | `muted` | Texto secundario, subtítulos, íconos de cabecera, cuerpo de tarjetas, estados de carga. |
| `faint()` | `faint` | Metadatos, pistas, íconos dentro de campos, `section_label`, contadores de "N más". |
| `accent()` | `accent` | Lo interactivo: enlaces, foco, selección, la sesión actual, jobs en curso. |
| `petal()` | `accent` (el mismo color) | La marca: punto vivo, `✿`, panel enfocado, resplandores, efectos. |
| `petal_active()` | `accent_active` | Variante más intensa de `petal`. |
| `champagne()` | `heading` | Títulos de tarjetas de herramientas, íconos de secciones de Configuración, código destacado. |
| `info()` | `info` | Modelos, chips de herramientas, estado "running". |
| `success()` / `danger()` / `warning()` | homónimos | Estados; también en bordes de tarjetas con `.opacity(0.35)`. |
| `panel()` | fondo teñido 4.5 % | Paneles, barra lateral. |
| `surface()` | fondo teñido 9 % | Tarjetas (`card`), teclas (`kbd`), botones secundarios. |
| `surface_hover()` | fondo teñido 15 % | Hover de filas y cabeceras plegables; fila activa en menús. |
| `raised()` | `raised` | Mensajes del usuario, fila seleccionada, opción activa de `segmented`. |
| `well()` | fondo apenas teñido | Hundido: código, salidas, campos de texto, marco de `segmented`. |
| `border()` | fondo teñido 20 %, alfa 0.69 (oscuro) / 0.8 (claro) | Borde de tarjetas y líneas finas de pantallas. |
| `border_strong()` | `border`, opaco | Campos, teclas, marco de pantallas e islas, separadores verticales. |
| `screen_bg()` | fondo teñido 5.5 %, opaco | Pantallas completas y paleta. |
| `popover_bg()` | fondo teñido 6.5 %, opaco | Islas, menús, tarjetas flotantes. |
| `backdrop()` | negro, alfa 0.44 (oscuro) / 0.25 (claro) | Velo detrás de pantallas (`scrim`) y color de la sombra de `island`. |

"Teñido" es `mix(bg, mix(fg, accent, 0.6), cantidad)`: el fondo inclinado hacia el texto con un toque del acento. `Palette` tiene además campos sin función pública (`window_bg`, `accent_deep`, `on_accent`, `danger_hover`, `danger_active`, `selection`, `scrollbar_hover`) que sólo usa `apply_theme`.

### Transparencia

`Palette::new(theme, clear)` recibe la transparencia de la ventana (`clear`, 0 = vidrio más sólido, 1 = más claro; por defecto `DEFAULT_TRANSPARENCY = 0.6`, ajustable con el slider de Configuración). Cada superficie de vidrio baja su alfa desde un valor sólido hasta un piso que mantiene el texto legible (`fade(solid, floor)`); por ejemplo `panel` va de 0.93 a 0.16 y `surface` de 0.94 a 0.4. `popover_bg` y `screen_bg` no cambian.

### gpui-component

`ui::apply_theme(cx)` cambia `ThemeMode` a claro u oscuro según `Palette::light` y reescribe los colores de gpui-component, así los componentes de serie ya combinan:

| gpui-component | Token |
| --- | --- |
| `primary`, `button_primary`, `ring`, `link`, `progress_bar`, `slider_bar` | `accent` (hover: `accent_active`; texto: `on_accent`) |
| `button`, `secondary`, `muted` | `surface` (hover `surface_hover`, activo `raised`) |
| `danger`, `button_danger` | `danger` |
| `list_hover` / `list_active` / `tab_active` | `surface_hover` / `raised` / `raised` |
| `popover` | `popover_bg` |
| `input`, `window_border`, `scrollbar_thumb` | `border_strong` |
| `title_bar`, `title_bar_border`, `tab_bar`, `tab`, `list`, `scrollbar` | transparente |
| `radius` / `radius_lg` | 8 px / 12 px |

### Claro y oscuro

Las vistas no ramifican por modo: usan tokens. Las diferencias viven en `Palette::new` (alfa de `window_bg`, `well` sin tinte en claro, alfa de `border`, `on_accent` blanco en claro, `backdrop` más suave) y en `apply_theme` (el `overlay` de gpui-component). Si algo realmente lo necesita, se lee con `themes::palette(|p| p.light)`.

## Tipografía

- **Fuente de interfaz**: la de gpui-component (`cx.theme().font_family`); la app no la cambia. `ui::island` la fija explícitamente para que un menú abierto desde una barra monoespaciada no la herede.
- **Monoespaciada**: `ui::MONO` es `"Menlo"` en macOS, `"Consolas"` en Windows y `"DejaVu Sans Mono"` en el resto. Se usa para rutas, valores de configuración, código, contadores, relojes y tokens. `well_box` y `kbd` ya la aplican.
- **Escala**: el rem es el `font_size` de gpui-component (16 px), así que `text_xs` = 12 px, `text_sm` = 14 px y `text_base` = 16 px.

| Tamaño | Peso | Dónde |
| --- | --- | --- |
| 10 px | `SEMIBOLD`, mayúsculas | `section_label` |
| 10.5 px | normal | `chip`, `kbd` |
| 11–12 px | normal | Pistas, filas de chips de estado, asuntos de herramientas en `MONO` |
| 13 px | `SEMIBOLD` | Título de tarjeta (en `champagne`); cuerpo de tarjetas en 13 normal |
| 14 px | normal / `SEMIBOLD` | Texto de mensajes y compositor; títulos de tarjetas de error y diálogos |
| `text_xs` | normal | Subtítulos de pantalla, filas, metadatos (el más usado) |
| `text_sm` | normal / `MEDIUM` | Títulos de filas, estados vacíos y de carga; título de la tarjeta de un chip |
| `text_base` | `SEMIBOLD` | Título de pantalla en `screen_header` |
| `text_lg` / `text_xl` | `SEMIBOLD` | Nombre de agente, héroe del onboarding |
| 17 px | `SEMIBOLD`, `petal` | Título del control de esfuerzo |
| 34 px / alto de línea 42 | `BOLD` | Encabezado del welcome |
| 72 px en adelante | `EXTRA_BOLD`, `petal` | La palabra del final del welcome |

`SEMIBOLD` es el peso de los títulos; `MEDIUM` para títulos pequeños dentro de tarjetas flotantes; `BOLD` y `EXTRA_BOLD` sólo en el welcome.

## Espacio y radios

| Radio | Medida | Se usa en |
| --- | --- | --- |
| `rounded_md` | 6 px | `chip`, `kbd`, opciones de `segmented` |
| `rounded_lg` | 8 px | `well_box`, `field`, marco de `segmented`, filas de listas, recuadros de íconos |
| `rounded_xl` | 12 px | `card`, paneles, barra lateral |
| `rounded_2xl` | 16 px | Panel de `screen`, paleta |
| `rounded(px(18.))` | 18 px | `island` |
| `ISLAND_ROW` | 12 px | Filas dentro de una `island` con `p(px(6.))`: sus esquinas siguen las de la isla |
| `rounded_full` / `rounded(px(999.))` | píldora | Puntos de estado, botones de enviar/detener, botón principal del welcome |

Medidas que se repiten:

| Dónde | Medida |
| --- | --- |
| Cuerpo de la ventana | `px_1p5`, `pb_1p5`, `gap_1p5` entre barra lateral y paneles; divisor de 6 px |
| `screen` | margen de 32 px alrededor del panel; ancho fijo o, sin ancho, `size_full` con `max_w` 1280 |
| `screen_header` | `px_5`, `py_3`, `gap_3` |
| Cuerpo de pantalla | `p_4`, o `px_4 pt_4` con secciones en `gap_3` |
| Pie de pantalla | `px_4`, `py_3`, `gap_2` |
| Cabecera de tarjeta | `px_3`, `py_2`, `gap_2`; cuerpo `px_3 pb_2` |
| Sección de Configuración | `card().p_4()` (`features/settings/layout.rs`) |
| Campos | `field` mide 34 px de alto; 30 px en barras compactas |
| Tarjeta de un chip | 360 px de ancho, `p_3`, lista con `max_h` 380 |

Anchos de pantalla actuales: Providers 640, Onboarding 720, Sessions y Usage 760, Configuración 820; Profiles, Memory y Agents usan `screen(None)` (Profiles lo limita a 1100 × 760).

## Superficies y componentes

Orden de superficies, de atrás hacia adelante: ventana (`window_bg`) → `panel` → `card` (`surface`) → `well_box`/`field` (`well`, hundido). `raised` marca lo seleccionado. `island` y `screen` flotan, opacas.

| Pieza | Qué es | Cuándo |
| --- | --- | --- |
| `card()` | Columna `rounded_xl`, `surface`, borde `border` | Tarjetas de herramientas, secciones de Configuración, tarjetas de uso. |
| `well_box()` | Columna `rounded_lg`, `well`, `MONO`, `text_xs` | Código, salidas de comandos, diffs, stderr. |
| `chip(text, color)` | Pastilla de 10.5 px: texto `color`, fondo `color.opacity(0.12)`, borde `color.opacity(0.22)` | Modelo, estado, conteos, "current", "N unsaved". |
| `kbd(text)` | Tecla de 20 px en `MONO` | Pistas de atajos: `esc`, `↵`, `⌘↵`. |
| `section_label(text)` | 10 px `SEMIBOLD` en `faint`, en mayúsculas | Encabezado de un grupo. |
| `field()` | Fila de 34 px, `well`, borde `border_strong` | Marco para `Input::new(..).appearance(false)`, con ícono opcional a la izquierda. |
| `segmented(id, labels, selected, on_select)` | Opciones de texto dentro de un `well` | Conmutadores de dos o tres opciones ("This project / All projects"). |
| `island(cx)` | Tarjeta flotante opaca, radio 18, borde `border_strong().opacity(0.8)`, sombra con `backdrop()` | Menús del compositor, tarjetas de chips, isla de tareas, grupos del onboarding. |
| `screen(width)` + `screen_header()` | Velo + panel centrado; fila de cabecera | Toda pantalla superpuesta (ver Patrones). |

```rust
// Sección de tarjeta con título y contenido hundido.
ui::card().p_4().gap_2()
    .child(ui::section_label("Task"))
    .child(ui::well_box().px_3().py_2().text_color(ui::fg()).child(task));

// Campo de búsqueda.
ui::field()
    .child(Icon::new(IconName::Search).small().text_color(ui::faint()))
    .child(Input::new(&self.filter).appearance(false).small());

// Menú flotante con filas que siguen las esquinas de la isla.
ui::island(cx).p(px(6.)).child(
    div().px_3().py_1p5().rounded(px(ui::ISLAND_ROW))
        .hover(|s| s.bg(ui::surface_hover()))
        .child(label),
);
```

Un menú de gpui-component que deba verse como la isla se abre con `Popover::appearance(false)` y su contenido envuelto en `ui::island(cx)` (ver `chip_popover` más abajo).

Utilidades de presentación en el mismo módulo: `split_path` (prefijo `~/a/b/` y hoja, para mostrar el prefijo atenuado), `tail` (últimas N líneas y cuántas se ocultaron) y `format_duration`.

## Movimiento

| Función | Duración | Curva | Efecto | Uso típico |
| --- | --- | --- | --- | --- |
| `appear(id, el)` | 220 ms | `ease_out_quint` | Opacidad 0→1 y `mt` de 6 px a 0 | Primera aparición: filas, contenido de Configuración, barra lateral, tarjetas de uso. |
| `appear_after(id, delay, rise, el)` | `delay` + 420 ms | ease-out-quint calculado a mano (la espera queda lineal) | Opacidad y `top` relativo desde `rise` px | Entradas escalonadas (welcome, pasos del onboarding). |
| `pop_in(id, el)` | 340 ms | `spring` | Opacidad y `top` relativo desde 12 px | Pantallas, diálogos, paleta. |
| `popover_in(id, el)` | 300 ms | `spring` | Sólo opacidad, sin cambiar el tamaño medido | Contenido dentro de una `island` o un popover. |
| `scrim(id)` | 220 ms | `ease_out_quint` | Velo `backdrop()` que aparece | Detrás de un panel (ya lo pone `screen`). |
| `pulse(id, el)` | 1400 ms, repetida | `pulsating_between(0.35, 1.0)` | Respira la opacidad | Indicadores vivos: punto de actividad, agentes corriendo, `✿` del pensamiento. |
| `spring(t)` | — | — | Resorte con ~3 % de sobrepaso | Dentro del animador para cualquier cosa que se abre. |

Pautas:

- **Una animación por id.** GPUI reproduce una animación una vez por `ElementId`. Para repetirla se cambia el id con un contador (`ElementId::NamedInteger("effort-knob".into(), moves)` en `features/session/view/effort.rs`, `NamedInteger("max-surge".into(), run)` en `surge.rs`); para que no se repita al reabrir algo, el id debe quedarse estable.
- **Escalonar con `appear_after`**: misma base y un hueco constante por paso (`self.entrance + STEP_GAP * (index + 1)` con `STEP_GAP = 110 ms` en `features/onboarding/sequence.rs`).
- **No reproducir al desplazarse**: la conversación sólo aplica `appear` a una fila durante `APPEAR_WINDOW` (300 ms) desde que se vio por primera vez (`features/session/view/transcript_list.rs`).
- **`spring` va en el animador, no como easing**: la easing de GPUI debe quedar en 0..=1 y el resorte se pasa de 1 (`pop_in` lo aplica dentro del closure).
- **Mover sin empujar**: `pop_in` y `appear_after` desplazan con `.relative().top(..)`, que no mueve a los hermanos del flex; `appear` usa `mt`, que sí afecta el layout durante 220 ms.
- **Efectos sólo de pintura**: los efectos grandes dibujan en un `canvas` con `window.paint_quad`, no reciben entrada ni cambian el layout. El temblor del surge se aplica como desplazamiento de la raíz de la ventana con `request_animation_frame`, sin envolverla en una animación, porque eso cambiaría la clave de cada elemento interior (`features/workspace/render.rs`). Un `Task` con un temporizador de la misma duración limpia el estado al terminar (`Workspace::play_surge`, `WelcomeView::enter`).

## Marca

`crates/app/src/shared/banner.rs` dibuja el banner de Gentle Shell (la rosa en braille y el logotipo "Gentle Shell" en pixel art) como píxeles reales. El arte, las paletas y el formato de `banner.json` vienen de `extensions/startup-banner.ts` de gentle-pi.

| API | Qué hace |
| --- | --- |
| `banner(config, scale, reveal)` | Rosa + logotipo a `scale` px por píxel (~175 px de ancho por unidad de escala). `reveal` en 0..=1 escribe el logotipo de izquierda a derecha y hace aparecer la rosa; se anima pasando `t` desde un `with_animation`. |
| `BannerConfig { show_rose, show_text_logo, palette }` | Qué partes mostrar y con qué paleta; `Default` muestra todo en `Pink`. |
| `Palette::{Pink, Cyan, Yellow, Green}` | Rosa, tinta y sombra del logotipo en RGB fijos (no dependen del tema). |
| `read_config(config_home)` | Lee `<config>/banner.json` (`showRose`, `showTextLogo`, `color`); lo que falte o esté mal vuelve al valor por defecto. |
| `pixel_rose(height, color)` | Sólo la rosa, en un color del tema (el héroe del onboarding la usa en `petal` o `success`). |
| `rose()` | Puntos y tamaño de la rosa, para otros efectos. |

El estilo pixel art comparte reglas: cuadrados planos pintados con `paint_quad`, ligeramente más pequeños que su celda para que se note la rejilla (`dot * 0.55` en la rosa, `dot * 0.9` en los sprites); sprites definidos como texto (`#` cuerpo, `o` línea) como `LIPS` y `HEART` en `features/welcome/finale.rs`; colores derivados de `petal()` cuando el efecto es de la app.

- **Final del welcome** (`features/welcome/finale.rs`, `FINALE = 5600 ms`): el banner se reescribe en cámara lenta, la rosa lanza 14 besos pixelados (uno de cada cuatro es un corazón en `petal`), "Sexyyyy" entra letra a letra con `spring` y la pantalla se disuelve desde `DISSOLVE = 0.86` con smoothstep.
- **Surge de esfuerzo máximo** (`shared/surge.rs`, `DURATION = 4400 ms`): un cometa de píxeles recorre el borde de la ventana, la ventana tiembla, el núcleo eclosiona y sale la rosa. Lo dispara `SessionEvent::MaxEffort` y lo reproduce `Workspace::play_surge`; `surge::overlay(run)` pinta y `surge::shake_at(elapsed)` da el desplazamiento. Sus colores (`spark`, `wine`) se derivan del tono de `petal()`.

## Íconos

Los íconos son `IconName` de `gpui_kit::assets` (el catálogo Lucide) y se dibujan con `Icon::new(IconName::X)`, casi siempre `.small()` o `.xsmall()`. El binario sólo incluye el paquete por defecto de gpui-kit más los nombres listados en `ExtraIcons` (`crates/app/src/shared/assets.rs`); `AppAssets` busca primero en `ExtraIcons` y después en `gpui_kit::assets::Assets`, y es la fuente registrada con `.with_assets(assets::AppAssets)` en `app/mod.rs`.

Para usar un ícono de Lucide que no se ve:

1. Agregar su nombre a la lista de `gpui_kit::assets::icon_assets!(ExtraIcons, [...])`, con un comentario si pertenece a una funcionalidad concreta.
2. Usarlo con `Icon::new(IconName::Nombre)`. No se copia ningún SVG: el macro toma el que trae `gpui-kit-assets`.

Un SVG propio, fuera de Lucide, necesitaría una rama más en `AppAssets::load`; hoy no existe ninguno.

Color de los íconos: `muted` en cabeceras de pantalla y tarjetas neutras, `faint` dentro de campos y en estados vacíos, el color del tono en estados y tarjetas de extensiones, `champagne` en secciones de Configuración.

## Patrones

### Pantalla superpuesta

Settings, Providers, Usage, Sessions, Profiles, Memory y Agents siguen la misma forma (`features/usage/view.rs`, `features/sessions/render.rs`). Onboarding usa `ui::screen` sin `screen_header`: su héroe hace de cabecera y `kbd("esc")` + cerrar flotan arriba a la derecha. La paleta (`features/palette/view.rs`) repite el marco a mano (`screen_bg`, `rounded_2xl`, `border_strong`, `shadow_2xl`) con alto fijo de 560 px para que no salte al filtrar.

```rust
let (backdrop, panel) = ui::screen(Some(760.));
backdrop.occlude().child(ui::pop_in(
    "usage-screen",
    panel.w_full().max_w(px(760.))
        .child(
            ui::screen_header()
                .child(Icon::new(IconName::Gauge).small().text_color(ui::muted()))
                .child(div().flex_shrink_0().text_base().font_weight(FontWeight::SEMIBOLD)
                    .text_color(ui::fg()).child("Usage"))
                .child(div().flex_1().min_w_0().truncate().text_xs()
                    .text_color(ui::muted()).child(subtitle))
                .child(ui::kbd("esc"))
                .child(Button::new("usage-close").ghost().xsmall().icon(Icon::new(IconName::X))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(UsageEvent::Close)))),
        )
        .child(div().id("usage-body").flex_1().min_h_0().overflow_y_scroll().p_4().child(body))
        .child(footer),
))
```

- Cabecera, en este orden: ícono `small` en `muted`, título `text_base SEMIBOLD`, subtítulo que ocupa el resto y se trunca, acciones opcionales (spinner, chip de conteo, `segmented`, botones `ghost().xsmall()` con tooltip), `kbd("esc")` y el botón de cerrar.
- El cuerpo es la única parte que se desplaza (`flex_1().min_h_0().overflow_y_scroll()` con id).
- La vista no maneja Esc: emite su evento `Close` y el `Workspace` la quita (`on_close_overlay`). El velo usa `.occlude()` para que no pasen clics a lo de atrás; sólo la paleta cierra al hacer clic en el velo.

### Pantalla completa: welcome

`features/welcome/` ocupa toda la ventana (bajo la barra de título de 36 px en Windows y Linux; en macOS deja una zona de arrastre bajo los semáforos).

- Fondo `screen_bg()` con un degradado desde arriba: `petal().opacity(0.10)` mientras se instala, `success().opacity(0.10)` cuando todo lo requerido está listo y `petal().opacity(0.18)` durante la entrada.
- Banner a escala 2.6 escribiéndose durante `INTRO` (1500 ms); al estar listo pasa a `Palette::Green` con `pop_in`. Encabezado (34 px `BOLD`) y pie entran con `appear_after` a `AFTER_INTRO` (1100 ms). Si la pantalla se vuelve a mostrar, `intro_done` dibuja todo en su sitio sin animar.
- Contenido centrado con `max_w(px(680.))`; pie de 88 px con `px_10`; ↵ ejecuta el botón principal.

### Chips de estado de la sesión

`features/session/view/activity.rs` dibuja la línea de actividad sobre el compositor. A la izquierda, mientras el agente trabaja: `pulse` de un punto `petal` de 6 px, reloj en `MONO` y `muted`, un separador de 1×10 px en `border_strong`, `✿ fase…` en `petal` y la herramienta actual truncada en `muted`. A la derecha, los chips.

Todo chip nuevo usa `chip_popover(id, icon, color, label, title, count, body)`: un `Button` `ghost().xsmall()` con `text_color(color)` que abre un `Popover` anclado `BottomRight`, de 360 px, con `appearance(false)`, contenido en `ui::island(cx)` + `ui::popover_in`. La tarjeta muestra ícono y título (`text_sm MEDIUM`), el conteo en `MONO` y una lista desplazable; `body` construye las filas sólo al abrir. Color del chip: `accent` si hay algo en curso, `muted` en reposo, `faint` para información pasiva (uso de tokens). En paneles más angostos que `TINY` los estados de extensiones se pliegan en un solo chip.

### Tarjetas de la conversación

Tarjetas de herramientas (`render_card` en `features/session/view.rs`) y de mensajes de extensiones (`features/session/view/cards.rs`) comparten forma:

- `ui::card().w_full().overflow_hidden()`; si el tono es peligro o aviso, `border_color(color.opacity(0.35))`.
- Cabecera `px_3 py_2 gap_2`: ícono `small` (tono o `muted`), título 13 px `SEMIBOLD` en `champagne` truncado, chip opcional (`info` en herramientas, el tono en extensiones), asunto en `MONO` 12 px `muted`, `+n` en `success` y `−n` en `danger`, chevron `faint` si se pliega. Una cabecera plegable tiene hover `surface_hover`.
- Cuerpo `px_3 pb_2`: texto 13 px en `muted`; salidas en `MONO` 12 px (`CustomBody::Tail`); Markdown con `TextView` en `fg`. Lo largo se corta y ofrece "N more lines" / "Show less".
- El tono (`Tone::{Success, Danger, Warning, Info, Muted}`) se decide con una función pura a partir del tipo y los `details` (`custom_look`), con pruebas.

### Botones y pies

| Regla | Ejemplo |
| --- | --- |
| Secundarios `ghost()` a la izquierda, un espaciador (`div().flex_1()` o `ml_auto()`), el principal `primary()` al final a la derecha. | Pie de Sessions, de Usage, del welcome, de Profiles. |
| Tamaños: `small()` en pies, `xsmall()` en cabeceras y barras, `large()` sólo en el welcome. | |
| Botones sólo con ícono llevan `tooltip`. | Refrescar en cabeceras. |
| Trabajo en curso: `.loading(busy)` y la etiqueta lo dice ("Installing…"); `.disabled(..)` cuando no se puede. | `welcome-primary`, `usage-refresh`, `welcome-recheck`. |
| Destructivo: `danger()` en confirmaciones (Discard, Delete), `danger().outline()` para detener agentes. | `features/profiles/view/confirm.rs`, `features/session/agents.rs`. |
| Confirmar en línea: `outline()`; cancelar: `ghost()` con "Cancel". | Renombrar perfil, modelo personalizado. |
| Píldora (`rounded(px(999.))`) sólo para enviar/detener del compositor y el principal del welcome. | `features/session/view/composer.rs` |

Pie de pantalla típico: `h_flex().px_4().py_3().gap_2().border_t_1().border_color(ui::border())`, con pistas (`kbd` + texto `text_xs muted`) o una línea de estado a la izquierda.

### Vacío, carga y error

| Estado | Forma |
| --- | --- |
| Cargando | `h_flex().gap_2().text_sm().text_color(ui::muted())` + `Spinner::new().small()` (en `muted` o `accent`) + "Reading sessions…". En línea, `Spinner::new().xsmall()`. |
| Recargando con datos | La lista se queda y el botón de refrescar muestra `.loading(..)`. |
| Vacío | `v_flex().items_center().py_8().gap_2()`, ícono en `faint` (`Inbox`, `Layers`) y una frase `text_sm muted` que dice qué hacer ("No profiles left. Create one with New."). |
| Error | Texto `text_sm` en `danger` con un botón `ghost` "Retry"; o una `card` con `border_color(ui::danger().opacity(0.35))`, ícono `CircleAlert`, `section_label` y el mensaje; con salida técnica, ésta va en un `well_box`. |
| Resultado de una acción | Línea `text_xs` con ícono `xsmall`: `muted` + `Info`, `success` + `CircleCheck`, `danger` + `CircleAlert` (pie de Profiles). |

## Hacer y no hacer

| Hacer | No hacer |
| --- | --- |
| Usar `ui::*` para todo color y las piezas de `ui.rs` para tarjetas, campos y chips. | Escribir `rgb(..)`/`hsla(..)` en una vista, salvo arte pixel con colores propios. |
| Separar zonas con espacio y cambio de superficie. | Agregar bordes o líneas dentro de paneles, tarjetas o el chrome. |
| Reservar `petal` para lo vivo y lo principal. | Usar `petal` o `accent` como fondo de bloques grandes. |
| Texto secundario en `muted`, metadatos en `faint`, valores en `MONO`. | Subir todo a `fg` o a `text_sm`. |
| Abrir pantallas con `screen` + `pop_in` y menús con `island` + `popover_in`. | Usar el marco de popover de serie o un velo propio. |
| Ids de animación estables; un contador en el id sólo para repetirla. | Ids que cambian en cada render (la animación se reinicia sin parar). |
| Efectos grandes en un `canvas`, sin entrada y con un `Task` que los limpia. | Envolver la raíz de la ventana en una animación. |
| Probar en un tema claro (p. ej. Catppuccin Latte) y con la transparencia al máximo. | Suponer modo oscuro o fondo opaco. |

## Checklist para una pantalla nueva

1. La vista vive en su funcionalidad (`features/<nombre>/`); lo que sea genérico y lo use más de una va a `shared/ui.rs`, no a la vista.
2. `ui::screen(Some(ancho))` (640–820) o `None`, envuelto en `ui::pop_in("<nombre>-screen", ..)` y con `.occlude()` en el velo.
3. Cabecera con `screen_header`: ícono `muted`, título `text_base SEMIBOLD`, subtítulo truncado, acciones `ghost().xsmall()` con tooltip, `kbd("esc")` y cerrar; la vista emite `Close` y la registra el `Workspace`.
4. Cuerpo desplazable con id; grupos en `card()` con `section_label`; campos con `field()` + `Input::appearance(false)`; conmutadores con `segmented`.
5. Estados de carga, vacío y error con las formas de arriba; nada queda en blanco mientras se lee.
6. Pie con secundarios `ghost` a la izquierda y un único `primary` a la derecha; `.loading` y `.disabled` reflejan el trabajo en curso.
7. Íconos nuevos agregados a `ExtraIcons`.
8. Animaciones sólo con `appear`/`appear_after`/`pop_in`/`popover_in`/`pulse`, con ids estables.
9. Revisada en un tema claro, en uno oscuro y con distintas transparencias.
