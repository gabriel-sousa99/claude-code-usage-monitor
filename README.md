![Windows](https://img.shields.io/badge/plataforma-Windows-blue)
[![Licença: MIT](https://img.shields.io/badge/Licenca-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

# Claude Code Usage Monitor (Fork Customizado)

Projeto para Windows que monitora o consumo do Claude Code diretamente na barra de tarefas.

## Sobre Este Fork

Este repositório é um fork customizado do projeto original Claude Code Usage Monitor.

Objetivos deste fork:

- Interface totalmente em português do Brasil
- Visual do widget ajustado (mais compacto e com estilo de cápsula)
- Customização visual diretamente no menu de contexto
- Remoção da funcionalidade de verificação e ação de atualizações no menu

## O Que Foi Alterado Neste Fork

1. Interface em pt-BR

- Idioma português do Brasil adicionado à localização
- Aplicação configurada para usar pt-BR como idioma principal
- Menus de customização traduzidos para português

2. Design do widget

- Altura ajustada para layout mais compacto
- Painel arredondado com melhor contraste visual
- Indicadores e textos refinados para leitura na barra de tarefas

3. Menu de customização

- Tamanho (px): Automático, 320x38, 360x44, 400x50, 440x56
- Cores: fundo, fonte e indicadores
- Borda: 0px, 1px, 2px, 3px
- Espaçamento:
  - Preenchimento: 0px, 4px, 8px
  - Margem: 0px, 2px, 4px

4. Atualizações

- Fluxo de verificação automática e manual de atualizações removido do menu

## Requisitos

- Windows 10 ou Windows 11
- Claude Code (CLI ou App) instalado e autenticado

WSL também é suportado para leitura de credenciais, quando aplicável.

## Como Executar

### Opção 1: desenvolvimento

```powershell
cargo run
```

### Opção 2: gerar executável release

```powershell
cargo build --release
```

Executável gerado em:

```text
target/release/claude-code-usage-monitor.exe
```

## Como Usar

Depois de iniciar o app:

- O widget aparece na barra de tarefas
- O ícone fica na bandeja do sistema
- Clique esquerdo no ícone: mostrar/ocultar widget
- Clique direito no widget ou no ícone: abrir menu

No menu, você pode:

- Atualizar os dados manualmente
- Ajustar a frequência de atualização
- Configurar a inicialização com o Windows
- Redefinir a posição
- Alterar o visual do widget (tamanho, cores, borda, margem e preenchimento)
- Encerrar o app

## Diagnóstico

Para coletar logs de diagnóstico:

```powershell
claude-code-usage-monitor --diagnose
```

Arquivo de log:

```text
%TEMP%/claude-code-usage-monitor.log
```

Configurações persistidas em:

```text
%APPDATA%/ClaudeCodeUsageMonitor/settings.json
```

## Privacidade

Este projeto é open source.

O app:

- Lê credenciais locais do Claude Code para autenticação
- Consulta endpoints da Anthropic para obter uso e limites
- Não envia credenciais para servidores de terceiros
- Não coleta telemetria própria

## Licença

MIT.
