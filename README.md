![Windows](https://img.shields.io/badge/plataforma-Windows-blue)
[![Licenca: MIT](https://img.shields.io/badge/Licenca-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

# Claude Code Usage Monitor (Fork Customizado)

Projeto em Windows para monitorar consumo do Claude Code direto na barra de tarefas.

## Sobre Este Fork

Este repositorio e um fork/customizacao do projeto original Claude Code Usage Monitor.

Objetivo deste fork:
- Interface totalmente em portugues do Brasil
- Visual do widget ajustado (mais compacto e com estilo de capsula)
- Customizacao visual direto no menu de contexto
- Remocao da funcionalidade de verificacao/acao de atualizacoes no menu

## O Que Foi Alterado Neste Fork

1. Interface em pt-BR
- Idioma portugues do Brasil adicionado na localizacao
- Aplicacao configurada para usar pt-BR como idioma principal
- Menus de customizacao traduzidos para portugues

2. Design do widget
- Altura ajustada para layout mais compacto
- Painel arredondado com melhor contraste visual
- Indicadores e textos refinados para leitura na barra de tarefas

3. Menu de customizacao
- Tamanho (px): Automatico, 320x38, 360x44, 400x50, 440x56
- Cores: fundo, fonte e indicadores
- Borda: 0px, 1px, 2px, 3px
- Espacamento:
  - Preenchimento: 0px, 4px, 8px
  - Margem: 0px, 2px, 4px

4. Atualizacoes
- Fluxo de verificacao automatica/manual de atualizacoes removido do menu

## Requisitos

- Windows 10 ou Windows 11
- Claude Code (CLI ou App) instalado e autenticado

WSL tambem e suportado para leitura de credenciais, quando aplicavel.

## Como Executar

### Opcao 1: desenvolvimento

```powershell
cargo run
```

### Opcao 2: gerar executavel release

```powershell
cargo build --release
```

Executavel gerado em:

```text
target/release/claude-code-usage-monitor.exe
```

## Como Usar

Depois de iniciar o app:
- O widget aparece na barra de tarefas
- O icone fica na bandeja do sistema
- Clique esquerdo no icone: mostrar/ocultar widget
- Clique direito no widget ou no icone: abrir menu

No menu, voce pode:
- Atualizar os dados manualmente
- Ajustar frequencia de atualizacao
- Configurar inicializacao com Windows
- Redefinir posicao
- Alterar visual do widget (tamanho, cores, borda, margem e preenchimento)
- Encerrar o app

## Diagnostico

Para coletar logs de diagnostico:

```powershell
claude-code-usage-monitor --diagnose
```

Arquivo de log:

```text
%TEMP%/claude-code-usage-monitor.log
```

Configuracoes persistidas em:

```text
%APPDATA%/ClaudeCodeUsageMonitor/settings.json
```

## Privacidade

Este projeto e open source.

O app:
- Le credenciais locais do Claude Code para autenticacao
- Consulta endpoints da Anthropic para obter uso/limites
- Nao envia credenciais para servidores de terceiros
- Nao coleta telemetria propria

## Licenca

MIT.
