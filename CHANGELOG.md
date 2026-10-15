# Changelog

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
