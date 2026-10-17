# Changelog

## 0.8.0 (2026-10-17)

Saída pela casa.

- Para quem o admin escolheu no painel do AI Router, as chamadas à Anthropic da conta Claude dedicada a essa pessoa saem pelo IP de casa dela. O computador abre uma conexão de saída (WebSocket em `wss://<roteador>/etus/home-egress`) e só repassa bytes TLS entre o roteador e a Anthropic: nunca vê o token da conta.
- `etus router setup --saida-casa` faz o setup normal e liga a saída pela casa neste computador. Precisa da chave deste computador pelo login (`sk-etd-...`): com `--chave` ou `OMNIROUTE_API_KEY`, recusa.
- `etus router saida-casa ligar`, `desligar` (pode rodar mais de uma vez) e `status` (também `home-egress on|off`). `etus router status` ganha a seção "Saída pela casa": se o agente está instalado e rodando, o estado (conectado, aguardando o admin, chave recusada, roteador sem a função), o IP de saída visto pela Anthropic, quando foi atualizado e o caminho do log.
- Por que um agente do sistema e não o hook `SessionStart` do Claude Code: o agente fica de pé independente de quantas sessões estão abertas (uma conexão por computador, não uma por sessão), volta sozinho depois de queda, suspensão ou troca de rede, e também atende o chat da Etus no navegador. O hook só rodaria quando uma sessão abre e precisaria de trava contra duplicatas.
- macOS: LaunchAgent do usuário `~/Library/LaunchAgents/io.etus.saida-casa.plist` (`RunAtLoad`, `KeepAlive`, `ProcessType` Background), ligado com `launchctl bootstrap gui/<uid>`. Linux: `~/.config/systemd/user/etus-saida-casa.service` com `Restart=always`, ligado com `systemctl --user enable --now`. Windows ainda não tem suporte.
- O agente roda `etus router tunel` (oculto na ajuda): lê a chave do cofre como o `etus router chave`, abre só conexões para `api.anthropic.com`, `console.anthropic.com`, `platform.claude.com` e `claude.ai` na porta 443 (qualquer outro destino é recusado sem consultar DNS), manda ping a cada 25 s, reconecta se nada chegar em 60 s e espera o tempo que o roteador pede em cada recusa.
- Estado em `saida-casa.json` e log curto em `saida-casa.log` (até 256 KB, depois `saida-casa.log.1`) na pasta de configuração do etus. Nenhum dos dois guarda a chave.
- `etus logout` também desliga a saída pela casa. `etus router setup` reinicia o agente, se ele estiver instalado, para usar a chave nova.

## 0.7.0 (2026-10-16)

Apps e agentes ativados por projeto.

- `etus ativar <alvo>...` (também `activate`): ativa no projeto um app (nomes de `etus apps`), um pacote (a categoria do app no hub, como `arbitragem`, ou o pacote fixo `stalk` com radar, personas e clone) ou um agente do Etus Agentes (`agente:<slug>`). Grava `.claude/skills/etus-<app>/SKILL.md` (e `.agents/skills` para o Codex, com `--agente` ou quando o projeto tem `AGENTS.md` ou `.agents`) com os comandos e o guia do app vindo de `etus_detalhar`; para um agente, `.claude/agents/etus-<slug>.md` com as instruções lidas pelo Extensões e os apps das ferramentas dele. Sem alvo, gera de novo o que já está ativo.
- O estado fica na chave `active` do `.etus.json` da raiz do projeto (a mesma do `etus setup`), sem mexer nas outras chaves. Os arquivos gerados vão para o `.git/info/exclude`.
- `etus desativar <alvo>...` (também `deactivate`) e `etus ativos [--json]` (também `active`).
- `etus skills limpar-globais` (também `clean-global`) apaga só as skills globais com a marca de `etus skills sincronizar`; o `etus ativar` sugere o comando quando elas existem.
- `etus skills sincronizar` continua igual.

Um setup só, editores completos e contexto mínimo.

- `etus setup` passa a cobrir o que só o `etus-skills setup` (Node) fazia: vários perfis (`etus setup codigo,design`, somados aos do `.etus.json`), plugins de cada perfil vindos do `profiles.json` do etus-skills (antes eram sempre `etus-core` e `etus-dev`), `--com-templates`, detecção do perfil pelos arquivos do projeto, o `.etus.json` gravado (`project`, `workspace`, `perfil`, `targets`, `mcp`), `--target` somando às plataformas gravadas (`--substituir-alvos` troca) e o merge do `.claude/settings.json` que mantém marketplace customizado e plugin desligado.
- `etus setup sincronizar` (também `sync`) aplica de novo o `.etus.json` sem login; `etus setup diagnosticar` (também `doctor`) confere `.etus.json`, plugins, regras, MCP `etus` e skills ativas e sai com 1 se houver falha.
- O MCP `etus` vai para cada editor: `.mcp.json` (Claude Code e Copilot), `.cursor/mcp.json`, `.codex/config.toml`, `.gemini/settings.json`, `.agents/mcp_config.json` (Antigravity) e o arquivo do usuário do Windsurf. Modo `quente` por padrão (`--mcp-modo completo|quente|compacto`); um `etus` que já existe só é trocado com `--mcp-modo`, e o setup avisa sobre o `etus` local do Claude Code e sobre o modo completo com o AI Router.
- `etus ativar` segue as plataformas do `.etus.json`: `.claude/skills` para o Claude Code e `.agents/skills` para Codex, Cursor, Copilot, Windsurf, Gemini CLI e Antigravity.
- O `setup` sugere `etus skills limpar-globais` quando há skills globais geradas.
- `etus setup --perfil codigo` não manda mais `?perfil=codigo` ao hub (o perfil do setup não é o perfil de acesso).
- A checagem de "fora da raiz do projeto" resolve a pasta existente mais próxima, para não recusar arquivo novo em pasta nova quando o projeto está atrás de um link (como `/var` no macOS).

A chave do AI Router pelo login.

- `etus router setup` sem `--chave` pega a chave do AI Router deste computador pela sessão do `etus login`: troca o token do login por um token de repasse do hub para o `router-panel` e pede ao painel do roteador uma chave com o nome da máquina, revogável em Minha chave. Sem permissão "Usar a IA", mostra o link para pedir acesso (código 5).
- A chave vai para o mesmo cofre do token do login (arquivo 0600 ou chaveiro), nunca para o `settings.json`: o Claude Code passa a lê-la com `"apiKeyHelper": "etus router chave"`, e o `ANTHROPIC_AUTH_TOKEN` em texto puro sai do arquivo (sem login, a chave antiga é levada para o cofre).
- `etus router chave`: imprime a chave do cofre para o `apiKeyHelper` (é um segredo; recusa num terminal sem `--force`).
- `--chave` e `OMNIROUTE_API_KEY` continuam como plano B, também guardados no cofre.
- `etus router status` mostra a origem da chave (login com o nome do computador, informada à mão ou texto puro antigo) e avisa quando `ANTHROPIC_AUTH_TOKEN` ou `ANTHROPIC_API_KEY` do shell passam na frente do `apiKeyHelper`.
- `etus logout` revoga a chave deste computador no roteador e a apaga do cofre.

## 0.6.1 (2026-10-16)

- `etus trabalho` grava `**/.claude/settings.local.json` no `.git/info/exclude` também quando o projeto fica numa pasta que é link (no macOS, `/var` aponta para `/private/var`). Antes, o caminho do arquivo que ainda não existia era comparado sem resolver a pasta e o exclude era pulado.

## 0.6.0 (2026-10-16)

Configuração nativa do Claude Code para o AI Router da Etus (OmniRoute) e rastreamento de trabalho integrado.

- `etus router setup` (também `configurar`, `init`): configura o Claude Code em `~/.claude/settings.json` com variáveis de ambiente do OmniRoute (`ANTHROPIC_BASE_URL`, `ANTHROPIC_AUTH_TOKEN`, modelos `dev-continuo` e `rapido`), hooks de telemetria e statusline nativa da CLI em Rust.
- Tratamento seguro de chave: suporte a entrada interativa sem eco no terminal via `dialoguer::Password`, mascaramento unicode seguro sem sobreposição de fatias, e resolução que apenas reutiliza tokens existentes se o `ANTHROPIC_BASE_URL` já for o do AI Router.
- Backups com permissão privada (0600) e retenção automática dos últimos 5 backups (`settings.json.bak.<timestamp>`).
- Detecção e preservação de statusline personalizada previamente existente do usuário.
- `etus router status` (também `etus router` sem subcomandos): inspeciona e reporta o status da integração, validando URL do roteador, hooks ativos e statusline.
- `etus trabalho` atualiza imediatamente `.claude/settings.local.json` no repositório ativo através de `refresh` e assegura exclusão no `.git/info/exclude`.
- Remoção do alias conflitante `work` em `etus trabalho`, preservando o comando externo do app Etus Work (`etus work`).
- Adição dos comandos internos (`router`, `roteador`, `ai-router`, `hook`, `hook-deferred`, `statusline`, `token`) à lista de comandos reservados (`BUILTIN_COMMANDS`).

## 0.5.0 (2026-10-15)

Skills dos seus apps para os agentes.

- `etus skills sincronizar` (também `etus skills sync`) gera uma skill por app que você pode usar na empresa ativa, a partir do catálogo atual: para que o app serve, cada comando com aliases, resumo, leitura ou altera (e destrutiva), permissão exigida e parâmetros (obrigatórios com tipo, opcionais pelo nome), receitas e cuidados do guia do app, e as regras de confirmação, `--yes` e ação necessária.
- Destino: `--agente claude|codex|todos` (padrão claude: `~/.claude/skills` ou `$CLAUDE_CONFIG_DIR/skills`; Codex: `~/.agents/skills`), `--projeto` (`.claude/skills` e `.agents/skills` na raiz do repositório ou na pasta atual) e `--pasta <dir>`.
- Pastas `etus-<app>` com uma linha marcadora; só essas são atualizadas ou removidas (app que saiu da sua lista). `etus-cli` é reservado para a skill geral do etus-skills. `SKILL.md` feito à mão com o mesmo nome fica intacto (ignorado). Idempotente, com `--dry-run` e `--json`.
- O guia vem de `etus_detalhar` com `app:<prefixo>` no hub (bloco `guide` do manifesto). Num hub sem guia, a skill sai só das ferramentas e o etus avisa no stderr.

## 0.4.1 (2026-10-15)

Binários públicos e instalador de uma linha.

- `etus atualizar-cli` baixa sem token do GitHub: lê `VERSION`, o binário e o `SHA256SUMS` de `https://github.com/evolution-foundation/etus-cli/releases/latest/download/`. `ETUS_CLI_DOWNLOAD_BASE` troca esse endereço (um espelho interno, por exemplo); com ele definido, uma falha no endereço vira erro em vez de cair no GitHub.
- O sha256 passa a ser obrigatório: sem `SHA256SUMS` (ou `<binário>.sha256`) que confira, nada é trocado.
- O caminho antigo continua como reserva: se os arquivos públicos não existirem, usa `GH_TOKEN`, `GITHUB_TOKEN` ou `gh auth token` na API do GitHub, como antes.
- `--verificar --json` traz `latest` sem o `v` (`0.4.1`).
- `scripts/release.sh <versão>` gera os binários na própria máquina: macOS Apple Silicon e Intel nativos (assinados ad hoc) e Linux x86_64 e aarch64 estáticos (musl) com `docker buildx --platform`, mais `SHA256SUMS`, `VERSION` e um `.sha256` por binário. Confere que cada um roda `--version` (os de Linux com `docker run`). Com `--publish`, cria a release `v<versão>` no GitHub.
- README: o instalador `curl -fsSL https://apps.etus.io/cli/install.sh | sh` vem primeiro.

## 0.4.0 (2026-10-14)

Ação necessária com link para a tela certa.

- Quando uma ferramenta responde `{"status": "action_required", "action": {...}}` (no `structuredContent` ou como JSON no texto), o etus mostra no stderr o título, a mensagem, o que falta e o link, mantém o resultado no stdout como sempre e sai com o código novo 5. Com `--json`, o erro no stderr traz também `action`.
- `--open` (também `--abrir`) abre o link no navegador. `etus config abrir-links sim|nao` (`open_actions` no arquivo) ou `ETUS_OPEN_ACTIONS` abrem sozinho, só quando há terminal.
- `--wait[=SEGUNDOS]` (também `--aguardar`) espera você concluir e repete a mesma chamada (2 s, depois a cada 5 s; padrão 300 s, no máximo 1800), como o `etus login` espera o navegador.
- Só abre links `https` (ou `http` em localhost); o resto é ignorado e tratado como resposta comum.
- `open`, `abrir`, `wait` e `aguardar` viram nomes reservados de flag: um parâmetro de ferramenta com esse nome passa a ser `--arg-<nome>`.
- `etus config` mostra `openActions`.

## 0.3.0 (2026-10-14)

Etiqueta de trabalho.

- `etus trabalho ENG-123` marca o item do Plane em que você está trabalhando, com `--fase`, `--projeto` (`hub:<id>`, `chat:<id>` ou `rotina:<id>`), `--empresa` e `--etiqueta`. Só com `--fase`, `--projeto` ou `--empresa`, altera o que já está marcado.
- O estado persiste no mesmo arquivo que o hook do instalador do roteador já lê (`<git dir>/etus-work` ou `~/.claude/etus-work`), então o item passa a ir no header `x-etus-work` do Claude Code a partir do próximo prompt.
- A empresa vem da empresa ativa do etus (cache do catálogo, sem rede).
- `etus trabalho mostrar` (também `etus trabalho` sozinho), `cabecalho` e `limpar`. O header segue a v2 do contrato (`docs/specs/work-context.md` no hub): `v=2` só com empresa ou projeto, ordem canônica e no máximo 256 caracteres.

## 0.2.1 (2026-10-14)

Token em arquivo e sem janelas do chaveiro.

- No macOS e no Linux o token passa a ficar em arquivo 0600 na pasta de configuração, como no `gh`, no `gcloud` e no `aws`. O binário é assinado ad hoc, então cada versão tinha outra identidade e o macOS pedia de novo permissão para o item `etus-cli` do chaveiro; num agente sem terminal, essa janela travava o comando. O Windows continua no Gerenciador de Credenciais, que não abre janela.
- Migração da 0.2.0: o primeiro comando rodado num terminal lê o token do chaveiro uma vez, grava no arquivo e apaga o item. Sem terminal, ou se a leitura falhar, o etus pede `etus login` (código 3) em vez de esperar.
- O chaveiro vira opção: `etus config armazenamento chaveiro|arquivo` (move o token) ou `ETUS_TOKEN_STORE=keychain|file`. Com o chaveiro escolhido no macOS e sem terminal (stdin ou stdout), o comando sai na hora com código 3.
- `etus token`: imprime um access token válido para scripts (`MCP_TOKEN=$(etus token)`; `--json` traz `expiresAt`). Recusa quando a saída é um terminal, a não ser com `--force`.
- `etus config` mostra `tokenStore` (arquivo ou chaveiro) e `tokenLocation`.

## 0.2.0 (2026-10-14)

Nomes em pt-BR primeiro.

- Cada app tem um nome canônico em pt-BR, tirado do nome do app: `tarefas`, `reunioes`, `mural`, `design`, `work`, `extensoes`, `rotinas`, `hub`. O prefixo MCP (`tasks`, `records`, `muraldereconheci`...), os nomes em inglês e o nome completo do app (`mural-de-reconhecimento`, `etus-design`) continuam aceitos como aliases. Também aceita o nome com acento ou maiúscula (`etus Reuniões`).
- Cada comando tem como nome canônico o título da ferramenta em pt-BR (`criar-tarefa`, `listar-tarefas`) quando o título é em pt-BR; senão, o nome da ferramenta em kebab-case. A outra forma (`create-task`, e também `create_task`) é sempre aceita. Os nomes são únicos dentro do app e não mudam com a ordem do catálogo.
- A ajuda mostra o nome que você digitou (`Uso: etus tarefas <COMANDO>` ou `Uso: etus tasks <COMANDO>`), as outras formas aceitas em `etus <app> <comando> --help` e a descrição completa ali; na lista de comandos a descrição é cortada no fim de uma palavra, com reticências.
- `etus apps` lista o nome canônico primeiro e os aliases depois; `etus apps --json` traz `app` (nome canônico), `prefix`, `aliases` e, por comando, `name`, `aliases`, `tool` e `writes`.

## 0.1.0 (2026-10-14)

Primeira versão.

- `etus login` pelo navegador com OAuth 2.1 do hub: registro dinâmico do cliente (guardado por hub), PKCE S256, retorno em `http://127.0.0.1:<porta>/callback`, opção `--no-browser` com colar o endereço de retorno e `--somente-leitura`.
- Token no chaveiro do sistema (macOS e Windows) ou em arquivo 0600; refresh com rotação feito sob trava de arquivo, para dois processos não queimarem o mesmo refresh token.
- Comandos gerados do `tools/list` do gateway `https://apps.etus.io/mcp`: um grupo por prefixo de app, comandos em kebab-case, aliases em pt-BR pelo título da ferramenta e pelo nome do app, flags a partir do JSON Schema, `--json` / `--input` para o objeto inteiro, validação local de obrigatórios e valores.
- Confirmação para ferramentas que alteram dados (`--yes` ou pergunta no terminal) e `--dry-run`.
- Saída legível por padrão, `--json` compacto e `--quiet`; códigos de saída 0, 1, 2, 3 e 4.
- `etus whoami`, `etus apps`, `etus empresa listar|usar`, `etus perfil mostrar|usar|limpar` (passa `?perfil=` ao gateway), `etus atualizar`, `etus skills buscar|ler`, `etus setup` (em breve), `etus completions`, `etus config` e `etus atualizar-cli` (releases do GitHub, com o token do `gh`).
- Cache do catálogo por hub, perfil e pessoa, com 5 minutos de validade.
