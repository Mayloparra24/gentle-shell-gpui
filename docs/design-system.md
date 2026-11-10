# Design system

How the app looks and which pieces it is built from. Everything lives in `crates/app/src/shared/`: `ui.rs` (tokens, components and motion), `themes.rs` (catalog and active theme), `banner.rs` and `surge.rs` (brand and pixel art effects) and `assets.rs` (icons). Views use these pieces instead of loose colors and sizes, so every screen looks like the others.

## Principles

These principles come from the current code. They are not aspirations.

| Principle | How it applies |
| --- | --- |
| **Translucent glass** | The window requests `WindowBackgroundAppearance::Blurred` and a transparent title bar (`crates/app/src/app/mod.rs`). Surfaces (`panel`, `surface`, `well`…) are the theme background, tinted and with alpha, so the blurred desktop shows through. What floats on top (menus, screens, palette) is opaque: `popover_bg` and `screen_bg` have alpha 1 at any transparency. |
| **No lines in the chrome** | The title bar has no border (`title_bar_border = clear` in `ui::apply_theme`). Panels and the sidebar have a transparent `border_1()`; only the focused panel of a split shows a `petal().opacity(0.45)` border (`features/workspace/view.rs`). Dividers between panels are a 6 px gap (`DIVIDER`) with a 2×40 px handle that appears in `petal()` only on hover. Inside, zones are separated by space and by the change of surface (`panel` → `surface` → `well`). |
| **Thin lines only in screens** | Inside an overlay screen there is a `ui::border()` line between header, body and footer (`screen_header` uses `border_b_1`, footers use `border_t_1`), and Settings rows use `border().opacity(0.5)`. No more are added. |
| **Petal as accent** | The brand pink (the theme's `accent`) is reserved for what is live and primary: the dot that pulses while the agent works, the `✿ phase…` text, the focused panel, primary buttons and the welcome glow. It is not used as a background fill. |
| **Dense but calm** | `text_xs` is the most used size and `muted` the most used color (more than `fg`). Semantic color goes on icons, chips and card borders, not on large blocks. Animations last 220–420 ms and overshoot by no more than ~3 %. |

## Color

### Where colors come from

```text
themes/*.json (Pi theme format)
  → themes::parse_pi_theme   → Theme (opaque colors 0xRRGGBB)
  → ui::Palette::new(theme, clear)   → roles with alpha, based on transparency
  → ACTIVE (global RwLock in themes.rs)
  → ui::fg(), ui::surface()…  (read via themes::palette(|p| …), no cx)
  → ui::apply_theme(cx)       → gpui-component colors (buttons, inputs, lists, menus)
```

- **Catalog** (`themes::load_catalog`): first the Gentle Shell themes embedded with `include_str!` from `crates/app/themes/` (`Gentleman-Cute`, `Gentleman-Sexy`, `Gentle`); themes installed in `<gentle-pi>/themes/` replace them by name. Then the user's themes in `<agent dir>/themes/` and finally the popular schemes in `SCHEMES`. Names are unique; the first one wins.
- **Initial theme** (`themes::pick_default`): the one saved in `dirs::data_dir()/gentle-shell-gpui/settings.json` (key `theme`), otherwise the `theme` from Pi's `settings.json`, otherwise `DEFAULT_THEME` (`"Gentleman-Cute"`).
- **Live changes**: `themes::select(index, cx)` and `themes::set_transparency(clear, persist, cx)` reinstall the palette, call `ui::apply_theme` and refresh all windows. Because tokens are read on every render, no view stores colors.

JSON fields used by `parse_pi_theme` (the rest of Pi's format is ignored, except for the terminal's ANSI table):

| `Theme` | JSON | If missing |
| --- | --- | --- |
| `bg` | `export.pageBg`, otherwise `vars.bg` | `0xffffff` light / `0x0e0e11` dark |
| `fg`, `muted`, `faint` | `colors.text`, `colors.muted`, `colors.dim` | mixes of `fg` and `bg` |
| `accent`, `accent_active` | `colors.accent`, `colors.borderAccent` | `fg`; mix of `accent` and `fg` |
| `raised` | `colors.userMessageBg` | `mix(bg, accent, 0.14)` |
| `border` | `colors.border` | `mix(bg, fg, 0.25)` |
| `heading` | `colors.toolTitle`, otherwise `colors.mdHeading` | `warning` |
| `info` | `vars.blue`, otherwise `colors.syntaxFunction` | ANSI blue |
| `success`, `danger`, `warning` | `colors.success`, `colors.error`, `colors.warning` | fixed values |
| `selection` | `colors.selectedBg` | `mix(bg, accent, 0.25)` |

`light` is decided by the luminance of `bg` (> 0.5), or that of `text` if there is no background.

### Tokens

All are argument-less functions in `crate::shared::ui` that return `Hsla`.

| Token | Comes from | Used for |
| --- | --- | --- |
| `fg()` | `fg` | Main text, titles, values. |
| `muted()` | `muted` | Secondary text, subtitles, header icons, card bodies, loading states. |
| `faint()` | `faint` | Metadata, hints, icons inside fields, `section_label`, "N more" counters. |
| `accent()` | `accent` | Interactive things: links, focus, selection, the current session, running jobs. |
| `petal()` | `accent` (the same color) | The brand: live dot, `✿`, focused panel, glows, effects. |
| `petal_active()` | `accent_active` | Stronger variant of `petal`. |
| `champagne()` | `heading` | Tool card titles, Settings section icons, highlighted code. |
| `info()` | `info` | Models, tool chips, "running" state. |
| `success()` / `danger()` / `warning()` | same names | States; also on card borders with `.opacity(0.35)`. |
| `panel()` | background tinted 4.5 % | Panels, sidebar. |
| `surface()` | background tinted 9 % | Cards (`card`), keys (`kbd`), secondary buttons. |
| `surface_hover()` | background tinted 15 % | Hover on rows and collapsible headers; active row in menus. |
| `raised()` | `raised` | User messages, selected row, active option of `segmented`. |
| `well()` | barely tinted background | Sunken: code, outputs, text fields, `segmented` frame. |
| `border()` | background tinted 20 %, alpha 0.69 (dark) / 0.8 (light) | Card borders and thin lines in screens. |
| `border_strong()` | `border`, opaque | Fields, keys, frame of screens and islands, vertical separators. |
| `screen_bg()` | background tinted 5.5 %, opaque | Full screens and palette. |
| `popover_bg()` | background tinted 6.5 %, opaque | Islands, menus, floating cards. |
| `backdrop()` | black, alpha 0.44 (dark) / 0.25 (light) | Veil behind screens (`scrim`) and shadow color of `island`. |

"Tinted" is `mix(bg, mix(fg, accent, 0.6), amount)`: the background leaned toward the text with a touch of the accent. `Palette` also has fields without a public function (`window_bg`, `accent_deep`, `on_accent`, `danger_hover`, `danger_active`, `selection`, `scrollbar_hover`) that only `apply_theme` uses.

### Transparency

`Palette::new(theme, clear)` receives the window transparency (`clear`, 0 = most solid glass, 1 = clearest; default `DEFAULT_TRANSPARENCY = 0.6`, adjustable with the Settings slider). Each glass surface lowers its alpha from a solid value down to a floor that keeps text readable (`fade(solid, floor)`); for example `panel` goes from 0.93 to 0.16 and `surface` from 0.94 to 0.4. `popover_bg` and `screen_bg` do not change.

### gpui-component

`ui::apply_theme(cx)` switches `ThemeMode` to light or dark according to `Palette::light` and rewrites the gpui-component colors, so the stock components already match:

| gpui-component | Token |
| --- | --- |
| `primary`, `button_primary`, `ring`, `link`, `progress_bar`, `slider_bar` | `accent` (hover: `accent_active`; text: `on_accent`) |
| `button`, `secondary`, `muted` | `surface` (hover `surface_hover`, active `raised`) |
| `danger`, `button_danger` | `danger` |
| `list_hover` / `list_active` / `tab_active` | `surface_hover` / `raised` / `raised` |
| `popover` | `popover_bg` |
| `input`, `window_border`, `scrollbar_thumb` | `border_strong` |
| `title_bar`, `title_bar_border`, `tab_bar`, `tab`, `list`, `scrollbar` | transparent |
| `radius` / `radius_lg` | 8 px / 12 px |

### Light and dark

Views do not branch by mode: they use tokens. The differences live in `Palette::new` (alpha of `window_bg`, untinted `well` in light, alpha of `border`, white `on_accent` in light, softer `backdrop`) and in `apply_theme` (gpui-component's `overlay`). If something really needs it, read it with `themes::palette(|p| p.light)`.

## Typography

- **UI font**: gpui-component's (`cx.theme().font_family`); the app does not change it. `ui::island` sets it explicitly so a menu opened from a monospaced bar does not inherit that font.
- **Monospaced**: `ui::MONO` is `"Menlo"` on macOS, `"Consolas"` on Windows and `"DejaVu Sans Mono"` elsewhere. It is used for paths, settings values, code, counters, clocks and tokens. `well_box` and `kbd` already apply it.
- **Scale**: the rem is gpui-component's `font_size` (16 px), so `text_xs` = 12 px, `text_sm` = 14 px and `text_base` = 16 px.

| Size | Weight | Where |
| --- | --- | --- |
| 10 px | `SEMIBOLD`, uppercase | `section_label` |
| 10.5 px | normal | `chip`, `kbd` |
| 11–12 px | normal | Hints, status chip rows, tool subjects in `MONO` |
| 13 px | `SEMIBOLD` | Card title (in `champagne`); card body in 13 normal |
| 14 px | normal / `SEMIBOLD` | Message and composer text; titles of error cards and dialogs |
| `text_xs` | normal | Screen subtitles, rows, metadata (the most used) |
| `text_sm` | normal / `MEDIUM` | Row titles, empty and loading states; title of a chip's card |
| `text_base` | `SEMIBOLD` | Screen title in `screen_header` |
| `text_lg` / `text_xl` | `SEMIBOLD` | Agent name, onboarding hero |
| 17 px | `SEMIBOLD`, `petal` | Effort control title |
| 34 px / line height 42 | `BOLD` | Welcome heading |
| 72 px and up | `EXTRA_BOLD`, `petal` | The word at the end of the welcome |

`SEMIBOLD` is the weight for titles; `MEDIUM` for small titles inside floating cards; `BOLD` and `EXTRA_BOLD` only in the welcome.

## Spacing and radii

| Radius | Size | Used in |
| --- | --- | --- |
| `rounded_md` | 6 px | `chip`, `kbd`, `segmented` options |
| `rounded_lg` | 8 px | `well_box`, `field`, `segmented` frame, list rows, icon boxes |
| `rounded_xl` | 12 px | `card`, panels, sidebar |
| `rounded_2xl` | 16 px | `screen` panel, palette |
| `rounded(px(18.))` | 18 px | `island` |
| `ISLAND_ROW` | 12 px | Rows inside an `island` with `p(px(6.))`: their corners follow the island's |
| `rounded_full` / `rounded(px(999.))` | pill | Status dots, send/stop buttons, welcome primary button |

Recurring sizes:

| Where | Size |
| --- | --- |
| Window body | `px_1p5`, `pb_1p5`, `gap_1p5` between sidebar and panels; 6 px divider |
| `screen` | 32 px margin around the panel; fixed width or, without width, `size_full` with `max_w` 1280 |
| `screen_header` | `px_5`, `py_3`, `gap_3` |
| Screen body | `p_4`, or `px_4 pt_4` with sections in `gap_3` |
| Screen footer | `px_4`, `py_3`, `gap_2` |
| Card header | `px_3`, `py_2`, `gap_2`; body `px_3 pb_2` |
| Settings section | `card().p_4()` (`features/settings/layout.rs`) |
| Fields | `field` is 34 px tall; 30 px in compact bars |
| Chip card | 360 px wide, `p_3`, list with `max_h` 380 |

Current screen widths: Providers 640, Onboarding 720, Sessions and Usage 760, Settings 820; Profiles, Memory and Agents use `screen(None)` (Profiles caps it at 1100 × 760).

## Surfaces and components

Surface order, back to front: window (`window_bg`) → `panel` → `card` (`surface`) → `well_box`/`field` (`well`, sunken). `raised` marks what is selected. `island` and `screen` float, opaque.

| Piece | What it is | When |
| --- | --- | --- |
| `card()` | `rounded_xl` column, `surface`, `border` border | Tool cards, Settings sections, usage cards. |
| `well_box()` | `rounded_lg` column, `well`, `MONO`, `text_xs` | Code, command outputs, diffs, stderr. |
| `chip(text, color)` | 10.5 px pill: `color` text, `color.opacity(0.12)` background, `color.opacity(0.22)` border | Model, state, counts, "current", "N unsaved". |
| `kbd(text)` | 20 px key in `MONO` | Shortcut hints that are not an action on their own: `↑↓`, `←→`. |
| `key_button(id, text)` | `kbd` with a pointer; hover lifts the text to `fg` and the border to `petal().opacity(0.6)` | A hint that stands for one action in a footer legend or dialog (`↵` run/submit, `esc` cancel); add `.on_click` doing what the key does. Screen headers don't show `esc`: they have the close X, and Esc still closes. |
| `section_label(text)` | 10 px `SEMIBOLD` in `faint`, uppercase | Heading of a group. |
| `field()` | 34 px row, `well`, `border_strong` border | Frame for `Input::new(..).appearance(false)`, with an optional icon on the left. |
| `segmented(id, labels, selected, on_select)` | Text options inside a `well` | Two- or three-option toggles ("This project / All projects"). |
| `island(cx)` | Opaque floating card, radius 18, `border_strong().opacity(0.8)` border, shadow with `backdrop()` | Composer menus, chip cards, task island, onboarding groups. |
| `screen(width)` + `screen_header()` | Veil + centered panel; header row | Every overlay screen (see Patterns). |

```rust
// Card section with a title and sunken content.
ui::card().p_4().gap_2()
    .child(ui::section_label("Task"))
    .child(ui::well_box().px_3().py_2().text_color(ui::fg()).child(task));

// Search field.
ui::field()
    .child(Icon::new(IconName::Search).small().text_color(ui::faint()))
    .child(Input::new(&self.filter).appearance(false).small());

// Floating menu with rows that follow the island's corners.
ui::island(cx).p(px(6.)).child(
    div().px_3().py_1p5().rounded(px(ui::ISLAND_ROW))
        .hover(|s| s.bg(ui::surface_hover()))
        .child(label),
);
```

A gpui-component menu that should look like the island is opened with `Popover::appearance(false)` and its content wrapped in `ui::island(cx)` (see `chip_popover` below).

Presentation utilities in the same module: `split_path` (prefix `~/a/b/` and leaf, to show the prefix dimmed), `tail` (last N lines and how many were hidden) and `format_duration`.

## Motion

| Function | Duration | Curve | Effect | Typical use |
| --- | --- | --- | --- | --- |
| `appear(id, el)` | 220 ms | `ease_out_quint` | Opacity 0→1 and relative `top` from 6 px to 0 | First appearance: rows, Settings content, sidebar, usage cards. |
| `appear_after(id, delay, rise, el)` | `delay` + 420 ms | ease-out-quint computed by hand (the wait stays linear) | Opacity and relative `top` from `rise` px | Staggered entrances (welcome, onboarding steps). |
| `pop_in(id, el)` | 340 ms | `spring` | Opacity and relative `top` from 12 px | Screens, dialogs, palette. |
| `popover_in(id, el)` | 300 ms | `spring` | Opacity only, without changing the measured size | Content inside an `island` or a popover. |
| `scrim(id)` | 220 ms | `ease_out_quint` | `backdrop()` veil fading in | Behind a panel (`screen` already adds it). |
| `pulse(id, el)` | 1400 ms, repeating | `pulsating_between(0.35, 1.0)` | Breathes the opacity | Live indicators: activity dot, running agents, thinking `✿`. |
| `spring(t)` | — | — | Spring with ~3 % overshoot | Inside the animator for anything that opens. |

Guidelines:

- **One animation per id.** GPUI plays an animation once per `ElementId`. To repeat it, change the id with a counter (`ElementId::NamedInteger("effort-knob".into(), moves)` in `features/session/view/effort.rs`, `NamedInteger("max-surge".into(), run)` in `surge.rs`); to keep it from replaying when something reopens, the id must stay stable.
- **Stagger with `appear_after`**: same base and a constant gap per step (`self.entrance + STEP_GAP * (index + 1)` with `STEP_GAP = 110 ms` in `features/onboarding/sequence.rs`).
- **Do not replay on scroll**: the conversation only applies `appear` to a row during `APPEAR_WINDOW` (300 ms) after it was first seen (`features/session/view/transcript_list.rs`).
- **`spring` goes in the animator, not as easing**: GPUI's easing must stay within 0..=1 and the spring goes past 1 (`pop_in` applies it inside the closure).
- **Move without pushing**: `appear`, `pop_in` and `appear_after` move with `.relative().top(..)`, which does not move flex siblings. Don't animate margins or sizes for an entrance. Since `.relative()` replaces the position, don't pass them an `.absolute()` element; wrap it instead.
- **Paint-only effects**: large effects draw in a `canvas` with `window.paint_quad`; they take no input and do not change layout. The surge shake is applied as an offset of the window root with `request_animation_frame`, without wrapping it in an animation, because that would change the key of every inner element (`features/workspace/render.rs`). A `Task` with a timer of the same duration clears the state when it ends (`Workspace::play_surge`, `WelcomeView::enter`).

## Brand

`crates/app/src/shared/banner.rs` draws the Gentle Shell banner (the braille rose and the "Gentle Shell" pixel art logotype) as real pixels. The art, the palettes and the `banner.json` format come from gentle-pi's `extensions/startup-banner.ts`.

| API | What it does |
| --- | --- |
| `banner(config, scale, reveal)` | Rose + logotype at `scale` px per pixel (~175 px wide per scale unit). `reveal` in 0..=1 writes the logotype left to right and fades in the rose; animate it by passing `t` from a `with_animation`. |
| `BannerConfig { show_rose, show_text_logo, palette }` | Which parts to show and with which palette; `Default` shows everything in `Pink`. |
| `Palette::{Pink, Cyan, Yellow, Green}` | Rose, ink and shadow of the logotype in fixed RGB (they do not depend on the theme). |
| `read_config(config_home)` | Reads `<config>/banner.json` (`showRose`, `showTextLogo`, `color`); anything missing or invalid falls back to the default. |
| `pixel_rose(height, color)` | Only the rose, in a theme color (the onboarding hero uses it in `petal` or `success`). |
| `rose()` | Points and size of the rose, for other effects. |

The pixel art style shares rules: flat squares painted with `paint_quad`, slightly smaller than their cell so the grid shows (`dot * 0.55` in the rose, `dot * 0.9` in sprites); sprites defined as text (`#` body, `o` outline) like `LIPS` and `HEART` in `features/welcome/finale.rs`; colors derived from `petal()` when the effect belongs to the app.

- **Welcome finale** (`features/welcome/finale.rs`, `FINALE = 5600 ms`): the banner rewrites itself in slow motion, the rose throws 14 pixelated kisses (one in four is a heart in `petal`), "Sexyyyy" enters letter by letter with `spring` and the screen dissolves from `DISSOLVE = 0.86` with smoothstep.
- **Max effort surge** (`shared/surge.rs`, `DURATION = 4400 ms`): a pixel comet runs along the window edge, the window shakes, the core hatches and the rose comes out. `SessionEvent::MaxEffort` triggers it and `Workspace::play_surge` plays it; `surge::overlay(run)` paints and `surge::shake_at(elapsed)` gives the offset. Its colors (`spark`, `wine`) are derived from the hue of `petal()`.

## Icons

Icons are `IconName` from `gpui_kit::assets` (the Lucide catalog) and are drawn with `Icon::new(IconName::X)`, almost always `.small()` or `.xsmall()`. The binary only includes gpui-kit's default bundle plus the names listed in `ExtraIcons` (`crates/app/src/shared/assets.rs`); `AppAssets` looks in `ExtraIcons` first and then in `gpui_kit::assets::Assets`, and it is the source registered with `.with_assets(assets::AppAssets)` in `app/mod.rs`.

To use a Lucide icon that does not show up:

1. Add its name to the `gpui_kit::assets::icon_assets!(ExtraIcons, [...])` list, with a comment if it belongs to a specific feature.
2. Use it with `Icon::new(IconName::Name)`. No SVG is copied: the macro takes the one shipped with `gpui-kit-assets`.

A custom SVG, outside Lucide, would need one more branch in `AppAssets::load`; none exists today.

Icon color: `muted` in screen headers and neutral cards, `faint` inside fields and in empty states, the tone color in states and extension cards, `champagne` in Settings sections.

## Patterns

### Overlay screen

Settings, Providers, Usage, Sessions, Profiles, Memory and Agents follow the same shape (`features/usage/view.rs`, `features/sessions/render.rs`). Onboarding uses `ui::screen` without `screen_header`: its hero acts as the header and the close X floats at the top right. The palette (`features/palette/view.rs`) repeats the frame by hand (`screen_bg`, `rounded_2xl`, `border_strong`, `shadow_2xl`) with a fixed height of 560 px so it does not jump while filtering.

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
                .child(ui::key_button("usage-esc", "esc")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(UsageEvent::Close))))
                .child(Button::new("usage-close").ghost().xsmall().icon(Icon::new(IconName::X))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(UsageEvent::Close)))),
        )
        .child(div().id("usage-body").flex_1().min_h_0().overflow_y_scroll().p_4().child(body))
        .child(footer),
))
```

- Header, in this order: `small` icon in `muted`, `text_base SEMIBOLD` title, subtitle that takes the remaining space and truncates, optional actions (spinner, count chip, `segmented`, `ghost().xsmall()` buttons with tooltip) and the close X (`ghost().xsmall()` with `IconName::X`). No `esc` badge in the header: Esc closes every screen anyway.
- The body is the only part that scrolls (`flex_1().min_h_0().overflow_y_scroll()` with an id).
- The view does not handle Esc: it emits its `Close` event and the `Workspace` removes it (`on_close_overlay`). The veil uses `.occlude()` so clicks do not pass through to what is behind; only the palette closes on a click on the veil.

### Full screen: welcome

`features/welcome/` takes up the whole window (below the 36 px title bar on Windows and Linux; on macOS it leaves a drag area below the traffic lights).

- `screen_bg()` background with a gradient from the top: `petal().opacity(0.10)` while installing, `success().opacity(0.10)` when everything required is ready and `petal().opacity(0.18)` during the entrance.
- Banner at scale 2.6, writing itself during `INTRO` (1500 ms); when ready it switches to `Palette::Green` with `pop_in`. Heading (34 px `BOLD`) and footer enter with `appear_after` at `AFTER_INTRO` (1100 ms). If the screen is shown again, `intro_done` draws everything in place without animating.
- Content centered with `max_w(px(680.))`; 88 px footer with `px_10`; ↵ runs the primary button.

### Session status chips

`features/session/view/activity.rs` draws the activity line above the composer. On the left, while the agent works: `pulse` of a 6 px `petal` dot, clock in `MONO` and `muted`, a 1×10 px separator in `border_strong`, `✿ phase…` in `petal` and the current tool, truncated, in `muted`. On the right, the chips.

Every new chip uses `chip_popover(id, icon, color, label, title, count, body)`: a `ghost().xsmall()` `Button` with `text_color(color)` that opens a `Popover` anchored `BottomRight`, 360 px wide, with `appearance(false)`, content in `ui::island(cx)` + `ui::popover_in`. The card shows icon and title (`text_sm MEDIUM`), the count in `MONO` and a scrollable list; `body` builds the rows only on open. Chip color: `accent` if something is running, `muted` at rest, `faint` for passive information (token usage). In panels narrower than `TINY`, extension states collapse into a single chip.

### Conversation cards

Tool cards (`render_card` in `features/session/view.rs`) and extension message cards (`features/session/view/cards.rs`) share a shape:

- `ui::card().w_full().overflow_hidden()`; if the tone is danger or warning, `border_color(color.opacity(0.35))`.
- Header `px_3 py_2 gap_2`: `small` icon (tone or `muted`), 13 px `SEMIBOLD` title in `champagne`, truncated, optional chip (`info` for tools, the tone for extensions), subject in `MONO` 12 px `muted`, `+n` in `success` and `−n` in `danger`, `faint` chevron if it collapses. A collapsible header has a `surface_hover` hover.
- Body `px_3 pb_2`: 13 px text in `muted`; outputs in `MONO` 12 px (`CustomBody::Tail`); Markdown with `TextView` in `fg`. Long content is cut and offers "N more lines" / "Show less".
- The tone (`Tone::{Success, Danger, Warning, Info, Muted}`) is decided by a pure function from the type and the `details` (`custom_look`), with tests.

### Buttons and footers

| Rule | Example |
| --- | --- |
| Secondary `ghost()` buttons on the left, a spacer (`div().flex_1()` or `ml_auto()`), the primary `primary()` last on the right. | Footer of Sessions, Usage, welcome, Profiles. |
| Sizes: `small()` in footers, `xsmall()` in headers and bars, `large()` only in the welcome. | |
| Icon-only buttons have a `tooltip`. | Refresh in headers. |
| Work in progress: `.loading(busy)` and the label says so ("Installing…"); `.disabled(..)` when the action is not possible. | `welcome-primary`, `usage-refresh`, `welcome-recheck`. |
| Destructive: `danger()` in confirmations (Discard, Delete), `danger().outline()` to stop agents. | `features/profiles/view/confirm.rs`, `features/session/agents.rs`. |
| Inline confirm: `outline()`; cancel: `ghost()` with "Cancel". | Rename profile, custom model. |
| Pill (`rounded(px(999.))`) only for the composer's send/stop and the welcome primary button. | `features/session/view/composer.rs` |

Typical screen footer: `h_flex().px_4().py_3().gap_2().border_t_1().border_color(ui::border())`, with hints (`kbd`/`key_button` + `text_xs muted` text) or a status line on the left.

### Empty, loading and error

| State | Shape |
| --- | --- |
| Loading | `h_flex().gap_2().text_sm().text_color(ui::muted())` + `Spinner::new().small()` (in `muted` or `accent`) + "Reading sessions…". Inline, `Spinner::new().xsmall()`. |
| Reloading with data | The list stays and the refresh button shows `.loading(..)`. |
| Empty | `v_flex().items_center().py_8().gap_2()`, icon in `faint` (`Inbox`, `Layers`) and a `text_sm muted` sentence that says what to do ("No profiles left. Create one with New."). |
| Error | `text_sm` text in `danger` with a `ghost` "Retry" button; or a `card` with `border_color(ui::danger().opacity(0.35))`, `CircleAlert` icon, `section_label` and the message; technical output, if any, goes in a `well_box`. |
| Action result | `text_xs` line with an `xsmall` icon: `muted` + `Info`, `success` + `CircleCheck`, `danger` + `CircleAlert` (Profiles footer). |

## Do and don't

| Do | Don't |
| --- | --- |
| Use `ui::*` for every color and the `ui.rs` pieces for cards, fields and chips. | Write `rgb(..)`/`hsla(..)` in a view, except pixel art with its own colors. |
| Separate zones with space and a change of surface. | Add borders or lines inside panels, cards or the chrome. |
| Reserve `petal` for what is live and primary. | Use `petal` or `accent` as the background of large blocks. |
| Secondary text in `muted`, metadata in `faint`, values in `MONO`. | Bump everything to `fg` or `text_sm`. |
| Open screens with `screen` + `pop_in` and menus with `island` + `popover_in`. | Use the stock popover frame or a custom veil. |
| Stable animation ids; a counter in the id only to repeat it. | Ids that change on every render (the animation restarts endlessly). |
| Large effects in a `canvas`, without input and with a `Task` that clears them. | Wrap the window root in an animation. |
| Test in a light theme (e.g. Catppuccin Latte) and with transparency at maximum. | Assume dark mode or an opaque background. |
| Make a key hint that stands for one action a `key_button` with the key's handler. | Leave an `esc`/`↵` cap that looks pressable but ignores the mouse. |

## Checklist for a new screen

1. The view lives in its feature (`features/<name>/`); anything generic used by more than one feature goes to `shared/ui.rs`, not to the view.
2. `ui::screen(Some(width))` (640–820) or `None`, wrapped in `ui::pop_in("<name>-screen", ..)` and with `.occlude()` on the veil.
3. Header with `screen_header`: `muted` icon, `text_base SEMIBOLD` title, truncated subtitle, `ghost().xsmall()` actions with tooltip, and the close X; the view emits `Close` and the `Workspace` handles it.
4. Scrollable body with an id; groups in `card()` with `section_label`; fields with `field()` + `Input::appearance(false)`; toggles with `segmented`.
5. Loading, empty and error states with the shapes above; nothing stays blank while reading.
6. Footer with secondary `ghost` buttons on the left and a single `primary` on the right; `.loading` and `.disabled` reflect work in progress.
7. New icons added to `ExtraIcons`.
8. Animations only with `appear`/`appear_after`/`pop_in`/`popover_in`/`pulse`, with stable ids.
9. Reviewed in a light theme, in a dark theme and with different transparencies.
