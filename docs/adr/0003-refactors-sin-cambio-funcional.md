# ADR-0003: refactors con conservación de comportamiento

- Estado: aceptada
- Fecha: 2026-11-09

## Contexto

La aplicación ya ofrece comportamiento amplio en varias plataformas. La reorganización arquitectónica no debe cambiar interacciones, persistencia, protocolo, atajos ni presentación observable.

## Decisión

Realizar la migración en etapas pequeñas y sin cambios funcionales. Antes de modificar lógica no suficientemente protegida se añadirá una prueba de caracterización que capture comportamiento relevante. Cada etapa debe ejecutar las comprobaciones aplicables y una prueba de humo del camino modificado.

No se añadirán pruebas de cableado, copias de campos, delegación trivial o texto fuente. Las pruebas cubrirán reglas, límites, transiciones, precedencia y errores observables.

## Consecuencias

- Los cambios serán más pequeños y revisables.
- Una extracción puede añadir pruebas antes de mover código.
- Las mejoras funcionales se separarán de los refactors arquitectónicos.
- Una etapa no se considerará verificada si el entorno impide ejecutar sus comprobaciones; esa limitación se reportará explícitamente.
