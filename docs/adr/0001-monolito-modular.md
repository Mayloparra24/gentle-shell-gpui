# ADR-0001: monolito modular por funcionalidades

- Estado: aceptada
- Fecha: 2026-11-09

## Contexto

La aplicación es un binario de escritorio GPUI con funcionalidades estrechamente integradas: ventanas, sesiones, archivos, terminal, configuración y actualizaciones. Parte del código ha crecido en módulos grandes, pero el producto no necesita despliegues independientes ni comunicación distribuida.

## Decisión

Mantener un monolito modular y organizarlo por funcionalidades. Cada funcionalidad tendrá una API pública pequeña y podrá dividir internamente estado, efectos y vista cuando esas responsabilidades sean suficientemente grandes.

Se conserva `pi-rpc` como crate independiente porque representa una frontera real: proceso hijo y protocolo, sin dependencia de GPUI.

## Consecuencias

- Los cambios entre funcionalidades siguen siendo simples y compilados conjuntamente.
- Las fronteras se expresan con privacidad de módulos y crates donde aporten valor.
- No se crea un crate por pantalla.
- Los módulos grandes se extraen por responsabilidades cohesivas, no sólo por número de líneas.
- La arquitectura puede evolucionar sin una migración total de una sola vez.
