# Documentación

Gentle Shell GPUI es una aplicación de escritorio en Rust ([GPUI](https://www.gpui.rs) + gpui-kit) que da una interfaz nativa a los agentes de [Gentle Shell](https://github.com/Gentleman-Programming/gentle-shell): cada pestaña de agente lanza `gentle-shell --mode rpc` y habla con él por JSONL. Esta carpeta explica cómo está construida, cómo se integra y cómo trabajar en ella.

## Qué leer según lo que vas a hacer

| Vas a… | Lee |
| --- | --- |
| Entender cómo está organizado el código y dónde va algo nuevo | [`architecture.md`](architecture.md) |
| Construir o cambiar una pantalla, un componente o una animación | [`design-system.md`](design-system.md) |
| Hablar con Gentle Shell, leer o escribir su configuración, o sumar un servicio externo | [`integrations.md`](integrations.md) |
| Compilar, probar sin tocar tu app real, abrir un PR o publicar una versión | [`development.md`](development.md) |
| Saber por qué se decidió algo | [`adr/`](adr/) |
| Instalar o usar la app | [`../README.md`](../README.md) |

## Mapa rápido

```text
crates/
├── app/       # el binario gentle-shell-gpui: GPUI, pantallas, efectos (archivos, procesos, red)
│   └── src/
│       ├── main.rs, app/, cli.rs   # arranque y composition root
│       ├── features/<nombre>/      # una carpeta por funcionalidad (session, workspace, welcome, …)
│       ├── platform/               # instancia única, íconos por sistema operativo
│       └── shared/                 # ui.rs (design system), themes, banner, assets, …
├── core/      # gentle-core: lógica pura, sin GPUI ni IO; aquí van los tests de reglas y parsers
└── pi-rpc/    # lanzador de Gentle Shell, homes, cliente JSONL, login de proveedores
```

## Decisiones vigentes

| ADR | En una línea |
| --- | --- |
| [0001](adr/0001-monolito-modular.md) | Un solo binario organizado por funcionalidades, con fronteras explícitas. |
| [0002](adr/0002-nucleo-puro-incremental.md) | La lógica pura vive en `gentle-core` y crece por comportamiento completo. |
| [0003](adr/0003-refactors-sin-cambio-funcional.md) | Los refactors no cambian comportamiento; los arreglos van en commits aparte. |

Para un agente de IA o alguien nuevo, el punto de entrada corto está en [`../AGENTS.md`](../AGENTS.md).
