# ADR-0002: núcleo puro incremental

- Estado: aceptada
- Fecha: 2026-11-09

## Contexto

La aplicación contiene parsers, modelos y transformaciones que pueden ejecutarse sin GPUI o IO, pero también mucho estado visual que legítimamente usa tipos del framework. Crear un crate `core` vacío o mover tipos acoplados a GPUI produciría adaptadores artificiales.

## Decisión

Extraer un crate `core` únicamente cuando exista lógica pura con una frontera estable. Mientras tanto, la lógica pura puede vivir en módulos internos de `app` y debe evitar dependencias innecesarias de GPUI, procesos y filesystem.

Son candidatos naturales los modelos del transcript, actividad, tareas, parsers de uso y normalización de perfiles. La extracción se hará por comportamiento completo, junto con sus pruebas.

## Consecuencias

- `core` no se convierte en una colección genérica de utilidades.
- No se introducen traits sólo para satisfacer una forma arquitectónica.
- La lógica pura resulta fácil de probar y reutilizar cuando la frontera sea real.
- Algunas funcionalidades continuarán combinando estado y vista cuando separarlas no mejore mantenibilidad.
