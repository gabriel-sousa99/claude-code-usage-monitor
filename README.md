# Claude Code Usage Monitor (Fork Customizado)

![Windows](https://img.shields.io/badge/plataforma-Windows-blue)
[![Licença: MIT](https://img.shields.io/badge/Licenca-MIT-yellow.svg)](LICENSE)

Widget leve para a barra de tarefas do Windows que monitora os limites de uso do Claude Code e o horário de reset. Também exibe uso do Codex, Google Antigravity, OpenCode Go e Cursor.

![Claude Code Usage Monitor rodando na barra de tarefas do Windows](.github/animation.gif)

## Sobre este fork

Este repositório é um fork de [CodeZeno/Claude-Code-Usage-Monitor](https://github.com/CodeZeno/Claude-Code-Usage-Monitor), sincronizado com a v2.10.21 do projeto original.

Diferenças em relação ao upstream:

- **pt-BR como idioma padrão**: quando o Windows não reporta um idioma reconhecido pelo app, o fallback é português do Brasil em vez de inglês (`src/localization/mod.rs`). Os outros 13 idiomas do upstream continuam disponíveis no menu (**Configurações → Idioma**), inclusive a opção "Padrão do sistema".
- **Sem verificação de atualizações**: o item de menu e a checagem automática em segundo plano foram desarmados. O app nunca faz requisição de rede para checar versão nem mostra prompt de atualização.
- **Cache de credenciais OAuth**: evita religar a distro WSL a cada ciclo de poll só para ler o token — o token fica em cache em memória enquanto for válido (ver "Sobre o cache OAuth" abaixo).
- **Diagnóstico não loga argumentos de CLI**: `--diagnose` grava o log sem incluir os argumentos de linha de comando recebidos pelo processo.

O upstream, entre a v1.3.1 (onde este fork havia parado) e a v2.10.21, deixou de ser só um monitor do Claude Code e passou a suportar múltiplos provedores (Codex, Antigravity, OpenCode, Cursor) com um Theme Studio completo para customização visual. Este fork adotou essa base inteira — a customização visual do widget (cor, borda, espaçamento, layout tipo cápsula), que antes era feita direto no menu de contexto, hoje é responsabilidade do **Theme Studio** (`claude-code-usage-monitor --dashboard`, aba de temas), não deste fork.

**Pendência conhecida, não portada**: o commit original do fork que exibia o countdown como "4h 32m" (horas e minutos) não foi reaplicado. No upstream atual esse texto é montado por uma função Rust (`format_usage_line`) compartilhada por todos os provedores, e as caixas de texto do tema não têm ajuste automático de largura — mudar o formato sem conseguir renderizar o widget arrisca cortar o texto na tela. Quem quiser esse formato deve validar visualmente no Windows antes de portar.

## Funcionalidades

- Mostra o uso atual e o tempo até cada limite resetar
- Suporta Claude Code, Codex, Google Antigravity, OpenCode Go e Cursor
- Fica na barra de tarefas do Windows, com controles rápidos na bandeja do sistema
- Suporta múltiplos monitores e inicialização com o Windows
- Intervalo de atualização, provedores e idioma configuráveis
- Temas prontos e um Theme Studio visual para layouts customizados
- Não coleta analytics nem telemetria

## Requisitos

- Windows 10 ou Windows 11
- Pelo menos um provedor suportado instalado e autenticado

As credenciais do Claude Code são detectadas via CLI, app desktop ou WSL. Os demais provedores são opcionais e podem ser ativados independentemente pelo dashboard.

## Instalação

Baixe `claude-code-usage-monitor.exe` na aba [Releases](../../releases) deste repositório, ou compile a partir do código-fonte (veja abaixo).

## Uso

Iniciar o monitor:

```powershell
claude-code-usage-monitor
```

Abrir o dashboard de configurações diretamente:

```powershell
claude-code-usage-monitor --dashboard
```

Use o dashboard para escolher provedores, mudar o intervalo de atualização, escolher um monitor, ativar a inicialização com o Windows ou customizar o widget (Theme Studio). No tema padrão, clique com o botão esquerdo no ícone de um provedor na bandeja para mostrar ou esconder o widget, e com o botão direito para abrir o menu.

## Configuração dos provedores

| Provedor | Configuração |
| --- | --- |
| Claude Code | Faça login pela CLI ou pelo app desktop do Claude Code. Credenciais do Windows e do WSL são detectadas automaticamente. |
| Codex | Instale e faça login na CLI do Codex, depois ative o Codex em **Provedores**. |
| Google Antigravity | Faça login no Antigravity, depois ative em **Provedores**. |
| OpenCode Go | Conecte uma conta OpenCode Go, configure as credenciais descritas abaixo, depois ative o OpenCode em **Provedores**. |
| Cursor | Faça login no Cursor, depois ative em **Provedores**. A sessão local é detectada automaticamente. |

Para o OpenCode Go, defina `OPENCODE_GO_WORKSPACE_ID` e `OPENCODE_GO_AUTH_COOKIE`, ou crie `%APPDATA%\opencode-go\config.json`:

```json
{
  "workspaceId": "wrk_01...",
  "authCookie": "seu-cookie-de-autenticacao-opencode"
}
```

O workspace ID faz parte da URL do workspace do OpenCode Go. O cookie de autenticação vem de uma sessão autenticada no navegador em `opencode.ai`. Defina `OPENCODE_GO_CONFIG_FILE` para usar outro caminho de configuração.

Para o Cursor, `CURSOR_SESSION_TOKEN` pode sobrescrever a sessão local detectada automaticamente.

## Sobre o cache OAuth (Claude Code via WSL)

Sem esse cache, cada ciclo de poll executaria `wsl.exe -d <distro> -- sh -lc "cat ~/.claude/.credentials.json"`. Com a distro parada, esse comando liga a VM inteira (systemd, docker, containerd, php-fpm etc.) só para ler ~1 KB, e ela desliga segundos depois — com um intervalo de poll de 5 minutos, isso significa boots frequentes da distro.

Como o token OAuth vale horas, este fork mantém o último token válido em cache de memória (`src/poller/claude.rs`) e só volta a ler o arquivo de credenciais quando o cache expira ou falha duas vezes seguidas. Isso não cobre o caminho de verificação de credenciais que roda enquanto o app está esperando o usuário corrigir um erro de autenticação (`wsl_credential_watch_signature`) — esse caminho já existia no upstream e continua ligando a distro nesse cenário específico, mais raro.

## Dados e privacidade

O monitor lê credenciais de login locais dos provedores ativados e envia requisições de uso diretamente aos serviços oficiais deles. Não há backend próprio, não coleta telemetria e não envia credenciais nem arquivos do projeto para lugar nenhum.

As credenciais são lidas sem modificar os arquivos dos provedores que as contêm. Credenciais do OpenCode Go salvas em arquivo de configuração JSON ficam em texto plano e devem ser protegidas como um cookie de sessão de navegador.

## Solução de problemas

Rode o diagnóstico com:

```powershell
claude-code-usage-monitor --diagnose
```

O log de diagnóstico é gravado em `%TEMP%\claude-code-usage-monitor.log`. As configurações da aplicação ficam em `%APPDATA%\ClaudeCodeUsageMonitor\settings.json`.

## Compilar a partir do código-fonte

### No Windows

Instale o [Rust](https://www.rust-lang.org/tools/install) 1.95 ou mais recente e rode:

```powershell
cargo build --release
```

O executável é gerado em `target\release\claude-code-usage-monitor.exe`.

### Cross-compile a partir do WSL/Linux

O projeto é Windows-only (usa a crate `windows` e recursos de PE), mas dá para compilar e rodar `cargo check`/`cargo build`/`cargo test --no-run` a partir do WSL para desenvolvimento, sem precisar abrir o Windows a cada mudança:

```bash
rustup target add x86_64-pc-windows-gnu
sudo apt install -y mingw-w64

mkdir -p ~/.local/mingw-shims/lib-ci
ln -sf /usr/bin/x86_64-w64-mingw32-windres ~/.local/mingw-shims/windres
ln -sf /usr/bin/x86_64-w64-mingw32-gcc-ar ~/.local/mingw-shims/ar
ln -sf /usr/x86_64-w64-mingw32/lib/libadvapi32.a ~/.local/mingw-shims/lib-ci/libAdvapi32.a

PATH="$HOME/.local/mingw-shims:$PATH" \
RUSTFLAGS="-L $HOME/.local/mingw-shims/lib-ci" \
cargo build --target x86_64-pc-windows-gnu
```

Os dois symlinks e o overlay de lib existem para contornar diferenças entre o toolchain do mingw-w64 do Linux e o que a crate `winres` (build de recursos do `.exe`) e a crate `windows` (nomes de biblioteca do Windows SDK) esperam encontrar por padrão em cross-compile.

Isso valida que o código compila e os testes montam (`cargo test --target x86_64-pc-windows-gnu --no-run`), mas **não roda os testes nem o app** — para isso, ainda é preciso Windows (ou um runner Wine configurado, não coberto aqui).

## Licença

Licenciado sob a [Licença MIT](LICENSE).
