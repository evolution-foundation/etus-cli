# etus

CLI global da plataforma ETUS. Um binário só, `etus`, gera os comandos de **todos os apps** a partir das ferramentas MCP que o hub libera para você: um app novo com ferramentas MCP vira `etus <app> <comando>` sem nenhuma mudança aqui.

Para agentes de código (Claude Code, Codex) é a interface mais econômica: o agente só lê `--help` quando precisa e usa `--json` para receber a resposta compacta.

Contrato completo: `docs/specs/cli.md` no repositório do hub (`etusdigital/nexus-platforms`).

## Instalar

```sh
curl -fsSL https://apps.etus.io/cli/install.sh | sh
etus --version
```

O instalador escolhe o binário da sua máquina (macOS Apple Silicon ou Intel, Linux x86_64 ou aarch64), confere o `SHA256SUMS` e coloca o `etus` no PATH.

Outras formas:

```sh
# Download direto da release (troque pelo nome da sua máquina)
base=https://github.com/evolution-foundation/etus-cli/releases/latest/download
curl -fsSLO "$base/etus-aarch64-macos" && curl -fsSLO "$base/SHA256SUMS"
shasum -a 256 -c SHA256SUMS --ignore-missing
chmod +x etus-aarch64-macos && sudo mv etus-aarch64-macos /usr/local/bin/etus
# etus-aarch64-macos | etus-x86_64-macos | etus-x86_64-linux | etus-aarch64-linux

# Com o GitHub CLI
gh release download --repo evolution-foundation/etus-cli --pattern etus-aarch64-macos --output etus
chmod +x etus && sudo mv etus /usr/local/bin/etus
```

Ou compile (Rust 1.89 ou mais novo):

```sh
cargo install --git https://github.com/evolution-foundation/etus-cli --locked
```

Depois, `etus atualizar-cli` baixa a versão mais nova sozinho, sem token: lê `VERSION`, o binário e o `SHA256SUMS` de `https://github.com/evolution-foundation/etus-cli/releases/latest/download/` e só troca o binário se o sha256 conferir. `ETUS_CLI_DOWNLOAD_BASE` aponta para outro endereço (um espelho, por exemplo). Se os arquivos públicos não existirem, usa `GH_TOKEN`, `GITHUB_TOKEN` ou `gh auth token` na API do GitHub, como reserva para um repositório privado.

## Entrar

```sh
etus login            # abre o navegador; você entra no hub e clica em Permitir
etus whoami           # quem está conectado e a empresa ativa
etus apps             # apps que você pode usar
```

- `etus login --no-browser` só mostra o endereço. Se o navegador estiver em outra máquina, cole no terminal o endereço da página de retorno (`http://127.0.0.1:<porta>/callback?...`).
- `etus login --somente-leitura` pede o escopo `mcp:read`: nenhum comando que altera dados aparece.
- Precisa da permissão "Conectar agentes externos" no hub, na empresa ativa.
- `etus logout` revoga a sessão no hub e apaga o token desta máquina.

### Onde o token fica

No macOS e no Linux o token fica em um arquivo com permissão 0600 na pasta de configuração (`tokens-<hash>.json`; `etus config` mostra o caminho), como fazem o `gh`, o `gcloud` e o `aws`. Nada de token aparece em log ou saída, exceto em `etus token`.

- **Por que não o chaveiro por padrão:** o binário é assinado ad hoc, então cada versão nova tem outra identidade e o macOS volta a pedir permissão para ler o item `etus-cli`. Num agente de código rodando `etus` sem terminal, essa janela trava o comando.
- **Quem vem da 0.2.0:** o primeiro comando rodado num terminal lê o token do chaveiro uma vez (pode aparecer uma última janela), grava no arquivo e apaga o item do chaveiro. Sem terminal, o etus não tenta ler o chaveiro: sai com código 3 pedindo `etus login`. Se a leitura falhar ou for negada, também pede `etus login`.
- **Usar o chaveiro mesmo assim:** `etus config armazenamento chaveiro` (move o token para lá) ou `ETUS_TOKEN_STORE=keychain` na sessão. Para voltar: `etus config armazenamento arquivo`. A variável vale mais que a configuração.
- **Chaveiro sem terminal:** com o chaveiro escolhido no macOS, qualquer comando sem terminal (stdin ou stdout que não é terminal, como em agentes e em `$(...)`) sai na hora com código 3 em vez de esperar a janela. Para scripts e agentes, use o arquivo.
- **Windows:** continua no Gerenciador de Credenciais por padrão, que não abre janela; `etus config armazenamento arquivo` troca para arquivo.
- **Linux:** não há chaveiro suportado; o token fica sempre no arquivo.

### Token para scripts

```sh
MCP_TOKEN=$(etus token)          # access token válido, renovado se estiver perto de expirar
etus token --json                # {"accessToken": "...", "expiresAt": 1760000000}
```

O token é um segredo: dá acesso à sua conta enquanto valer. Não cole em chat, log, issue nem arquivo versionado. Para não ficar visível na tela, `etus token` se recusa a imprimir quando a saída é um terminal (código 2); `--force` imprime mesmo assim. Útil para suítes de avaliação e scripts que chamam o gateway MCP direto.

## Usar

```sh
etus tarefas --help                                  # comandos do app
etus tarefas listar-tarefas --help                   # descrição completa e parâmetros
etus tarefas listar-tarefas --view today --today 2026-10-14
etus tarefas criar-tarefa --title "Revisar contrato" --due-date 2026-10-20 --yes
etus reunioes buscar-reunioes --q "preço" --json
etus mural --help
etus empresa listar
etus empresa usar etus                               # id, slug ou nome
etus skills buscar "planilha de custos"
etus skills ler <id>
```

Nomes, pt-BR primeiro:

| App | Nome | Também aceita |
|---|---|---|
| Tarefas | `tarefas` | `tasks` |
| Reuniões | `reunioes` | `records`, `meetings`, `gravador` |
| Mural de Reconhecimento | `mural` | `muraldereconheci`, `mural-de-reconhecimento` |
| Etus Design | `design` | `etus-design` |
| Etus Work | `work` | `etus-work` |
| Extensões de IA | `extensoes` | `extensions`, `ext` |
| Rotinas | `rotinas` | `routines` |
| ETUS Platforms | `hub` | `plataforma`, `platforms` |

Um app novo ganha o nome do próprio app no hub, sem acento e até o primeiro "de" (`Agenda de Salas` vira `agenda`); o prefixo MCP vira alias. Os comandos seguem o título da ferramenta quando ele é em pt-BR (`Criar tarefa` vira `criar-tarefa`); senão, o nome da ferramenta (`tasks__create_task` vira `create-task`). A outra forma sempre funciona: `etus tasks create-task` e `etus tarefas criar-tarefa` chamam a mesma ferramenta. `etus apps` mostra os nomes e os aliases, e `etus <app> <comando> --help` mostra as outras formas aceitas.

Argumentos:
- flags geradas do JSON Schema: `--project-id x`, números, `--done` ou `--done false`, listas repetindo a flag (`--tag a --tag b`) ou em JSON (`--tag '["a","b"]'`), objetos em JSON, `null` para apagar campos que aceitam nulo;
- ou o objeto inteiro: `--json '{"title":"x"}'`, `--input args.json`, `--input -` (stdin). Flags somam e têm prioridade sobre o objeto;
- obrigatórios e valores permitidos são conferidos antes de chamar o hub.

Ferramentas que alteram dados pedem confirmação no terminal ou `--yes` (sem terminal, sem `--yes`, o comando é recusado com código 2). `--dry-run` mostra a chamada sem executar.

## Para agentes

```sh
etus ativar radar               # só os apps que importam neste projeto, com o guia de cada um (veja abaixo)
etus skills sincronizar         # uma skill por app que você usa, na pasta do agente (veja abaixo)
etus --help                     # apps disponíveis (do cache)
etus <app> --help               # comandos do app, os que alteram marcados com [altera]
etus <app> <comando> --help     # parâmetros, exemplo e se pede --yes
etus <app> <comando> ... --json # resposta em JSON compacto
```

Com `--json`, a saída é o `structuredContent` da ferramenta ou o texto dela já decodificado; erros vão para o stderr como `{"error":{"kind","message","exitCode"}}`.

| Código | Significado |
|---|---|
| 0 | ok |
| 1 | a ferramenta falhou, ou falha de rede ou do hub |
| 2 | uso errado: flag inválida, obrigatório faltando, escrita sem `--yes`, cancelado |
| 3 | precisa entrar: rode `etus login` |
| 4 | sem permissão: ferramenta não liberada, conexão só de leitura, sem "Conectar agentes externos" |
| 5 | ação necessária: a ferramenta pediu que você faça algo numa tela do hub (autorizar, conectar, entrar, pedir acesso); o link está no stderr e no JSON do stdout |

## Ação necessária: o link para a tela certa

Alguns passos precisam de um clique seu por definição: autorizar um app a agir em seu nome, conectar uma conta (Google, GitHub, Slack), entrar, pedir uma permissão a um admin. Nesses casos a ferramenta não falha: devolve um resultado normal com este `structuredContent` (contrato da plataforma, `docs/specs/action-required.md` no hub):

```json
{
  "status": "action_required",
  "action": {
    "kind": "consent | connect | login | permission",
    "title": "Conectar o Google",
    "message": "Conecte sua conta Google para ler a agenda.",
    "url": "https://apps.etus.io/...",
    "missing": ["google"],
    "expiresAt": "2026-10-14T18:00:00Z"
  }
}
```

O etus também reconhece o mesmo objeto quando ele vem só como JSON no texto da ferramenta. Ao receber:

- **stdout** fica igual a qualquer outra resposta: o resultado da ferramenta (com `--json`, o objeto acima compacto), para o agente ler o link;
- **stderr** mostra o título, a mensagem, o que falta e o link (`Abra: https://...`); com `--json`, o erro traz também `action`: `{"error":{"kind":"action_required","message","exitCode":5,"action":{...}}}`;
- **código de saída 5**, para scripts e agentes detectarem sem ler o texto.

O etus nunca faz a ação por você: ele só mostra o link (e abre o navegador, se você pedir). Só abre links `https` (ou `http` em localhost).

```sh
etus <app> <comando> --open           # abre o link no navegador (também --abrir)
etus <app> <comando> --wait           # mostra o link, espera você concluir e repete a chamada (até 300 s)
etus <app> <comando> --wait=600 --open
etus config abrir-links sim            # abre sozinho quando há terminal (nao para desligar)
```

- `--open` abre sempre, com ou sem terminal. Sem a flag, o link só abre sozinho com `etus config abrir-links sim` (`open_actions` no arquivo) ou `ETUS_OPEN_ACTIONS=1`, e ainda assim só quando stdin e stderr são um terminal; agentes rodando sem terminal nunca abrem o navegador sem `--open`.
- `--wait[=SEGUNDOS]` (também `--aguardar`) repete a mesma chamada, com os mesmos argumentos e a mesma confirmação, a primeira vez depois de 2 s e depois a cada 5 s, até a resposta não ser mais `action_required` ou o tempo acabar (padrão 300 s, no máximo 1800). Concluiu: imprime o resultado final e sai com o código dele. O tempo acabou: imprime o último resultado e sai com 5. Num agente, só use `--wait` quando a pessoa estiver por perto para clicar.
- As flags valem antes do app (`etus --open tarefas ...`) ou depois do comando. Por isso `open`, `abrir`, `wait` e `aguardar` passam a ser nomes reservados: um parâmetro de ferramenta com esse nome vira `--arg-open`, como já acontecia com `json` e `yes`.

## Skills dos seus apps para os agentes

`etus skills sincronizar` (também `etus skills sync`) gera uma skill por app que você pode usar na empresa ativa, para o agente saber de cara o que cada app faz e quais comandos chamar, sem redescobrir com `--help` a cada conversa. A regra geral do `etus` (login, empresa, JSON, códigos de saída, confirmação) fica na skill escrita à mão `etus-cli`, do pacote `etus-core` do etus-skills.

```sh
etus login
etus skills sincronizar                      # Claude Code: ~/.claude/skills/etus-<app>/SKILL.md
etus skills sincronizar --agente codex       # Codex: ~/.agents/skills/etus-<app>/SKILL.md
etus skills sincronizar --agente todos       # os dois
etus skills sincronizar --projeto            # pastas do repositório: .claude/skills e .agents/skills
etus skills sincronizar --pasta ./skills     # uma pasta qualquer (vale mais que --agente e --projeto)
etus skills sincronizar --dry-run            # mostra o que mudaria, sem gravar
etus skills sincronizar --json               # {"pastas","criadas","atualizadas","removidas","iguais","ignoradas"}
```

- **Do seu catálogo, na hora.** Renova o catálogo como `etus atualizar`, então respeita a empresa ativa, o `--perfil` e o login só leitura: só entram os apps e comandos liberados para você.
- **O que cada skill traz.** Para que o app serve, como chamar, cada comando com aliases, resumo, se lê, altera ou é destrutivo, a permissão exigida e os parâmetros (obrigatórios com o tipo; opcionais pelo nome), as receitas e os cuidados que o app declara no manifesto (bloco `guide`) e as regras de confirmação e de ação necessária. App com mais de 60 comandos lista só os obrigatórios.
- **Guia do app.** Vem de `etus_detalhar` com `app:<prefixo>`, no hub. Num hub que ainda não serve o guia, a skill sai só das ferramentas, sem receitas nem permissões, e o etus avisa no stderr.
- **Nome e dono.** A pasta é `etus-<app>`, com o mesmo nome de `etus apps`. `etus-cli` é reservado para a skill geral: nunca é gravada nem apagada aqui. Toda skill gerada tem a linha `<!-- gerado por etus skills sincronizar: não edite, rode o comando de novo -->` logo depois do frontmatter.
- **Só mexe no que gerou.** Atualiza só o que mudou e remove só as pastas `etus-*` com essa linha cujo app saiu da sua lista (perdeu acesso, app removido, outra empresa). Um `SKILL.md` com o mesmo nome feito à mão fica como está e aparece como ignorado. Pastas que são links simbólicos não são seguidas.
- **Idempotente.** Sem data no conteúdo: rodar de novo sem mudança no hub não grava nada.
- Rode de novo depois de `etus empresa usar`, de ganhar ou perder acesso a um app, ou quando um app publicar versão nova. Depois de gravar, abra uma conversa nova no agente para ele carregar as skills.

`etus skills limpar-globais` (também `clean-global`) apaga de `~/.claude/skills` e `~/.agents/skills` só as pastas `etus-<app>` com a linha de `etus skills sincronizar`; as feitas à mão e a `etus-cli` ficam. Aceita `--dry-run` e `--json`.

## Ativar apps e agentes por projeto

As skills globais entram em toda sessão, de qualquer projeto. `etus ativar` (também `activate`) põe só o que importa dentro do projeto:

```sh
etus ativar radar personas          # apps, pelos nomes de `etus apps`
etus ativar arbitragem              # um pacote: a categoria do app no hub
etus ativar stalk                   # pacote fixo radar, personas e clone (quando nenhuma categoria do hub tem esse nome)
etus ativar agente:stalk            # um agente do Etus Agentes e os apps das ferramentas dele
etus ativar                         # gera de novo tudo o que já está ativo
etus ativar --agente todos radar    # Claude Code e Codex (padrão: Claude, mais Codex se o projeto tem AGENTS.md ou .agents)
etus ativos [--json]                # o que está ativo e por quê (também `active`)
etus desativar arbitragem           # tira e apaga os arquivos gerados (também `deactivate`)
```

- **Raiz e estado.** A raiz é a do repositório git (ou a pasta atual fora de um), a mesma do `etus setup`. O que está ativo fica na chave `active` do `.etus.json` da raiz (`apps`, `packs`, `agents`); as outras chaves do arquivo ficam como estão. O `.etus.json` é versionável: o time ativa a mesma lista.
- **App.** Grava `.claude/skills/etus-<app>/SKILL.md` (e `.agents/skills/etus-<app>/SKILL.md` para o Codex) com a mesma geração de `etus skills sincronizar`: comandos, permissões, e o guia do app (para que serve, receitas e cuidados) vindo de `etus_detalhar` com `app:<prefixo>`. App sem guia publicado sai só com os comandos, e o `etus ativar` diz quais.
- **Pacote.** Uma categoria de apps do hub (`arbitragem`, `produtividade`...), ou o pacote fixo `stalk` (radar, personas e clone) quando nenhuma categoria tem esse nome. A lista de membros é gravada no `.etus.json`.
- **Agente.** `agente:<slug>` (ou `agent:<slug>`) lê o agente pelo Extensões (`extensoes__catalogo_ler` com `agente:agentes:<slug>`), grava `.claude/agents/etus-<slug>.md` com as instruções dele e ativa os apps dos prefixos das ferramentas dele. Sem acesso, o etus mostra o link da ação necessária (código 5) ou o motivo (código 4).
- **Arquivos gerados fora do git.** Cada arquivo gerado entra no `.git/info/exclude`, como o `etus trabalho` faz com o `settings.local.json`: o conteúdo depende da empresa ativa e do acesso de cada pessoa, então cada um gera o seu a partir do `.etus.json`. Arquivo que o repositório já versiona fica versionado.
- **Só mexe no que gerou.** Os arquivos levam a linha `<!-- gerado por etus ativar: não edite, rode o comando de novo -->`. Uma skill de `etus skills sincronizar --projeto` com o mesmo nome é atualizada, mas só as de `etus ativar` são apagadas; as feitas à mão aparecem como ignoradas.
- **Dica.** Se houver skills globais de `etus skills sincronizar`, o fim do `etus ativar` sugere `etus skills limpar-globais`.

## Etiqueta de trabalho

`etus trabalho` marca em qual item do Plane você está trabalhando, para o uso de IA ser medido por empresa, projeto e item no painel do roteador. Contrato completo: `docs/specs/work-context.md` no repositório do hub.

```sh
etus trabalho ENG-123                         # marca o item; empresa = empresa ativa do etus
etus trabalho ENG-123 --fase review           # plan, impl, review ou qa
etus trabalho ENG-123 --projeto hub:<id>      # projeto da Etus: hub:<id>, chat:<id> ou rotina:<id>
etus trabalho ENG-123 --empresa <id>          # outra empresa (id da empresa no hub)
etus trabalho --etiqueta estudo-cache         # trabalho fora do Plane
etus trabalho --fase qa                       # só troca a fase do que já está marcado
etus trabalho                                 # o mesmo que `etus trabalho mostrar`
etus trabalho mostrar                         # estado e a linha do header que vai sair
etus trabalho cabecalho                       # só a linha x-etus-work, para hooks e scripts
etus trabalho limpar                          # apaga a marca (volta ao que o branch disser)
```

- **Persiste até mudar.** Dentro de um repositório, o estado fica em `<git dir>/etus-work` (um por worktree); fora, em `~/.claude/etus-work` (ou `$CLAUDE_CONFIG_DIR/etus-work`). O formato é o mesmo do utilitário do instalador do roteador (`issue`, `phase`, `tag`, `workspace`), com as chaves novas `company` e `project`.
- **Empresa automática.** Vem da empresa ativa guardada no cache do catálogo (`etus empresa usar`), sem chamar a rede. Sem login nem `--empresa`, fica vazia.
- **Item pelo branch.** Sem item marcado, vale a chave escrita no branch (`feat/eng-123-upload` vira `ENG-123`), como no hook.
- **Versão do header.** `v=2` só quando há empresa ou projeto (`c`, `pj` e `via`); senão `v=1`, que todo leitor antigo entende. A linha tem no máximo 256 caracteres: se passar, cortam-se a etiqueta, o repositório e o workspace, nunca empresa, projeto ou item.
- `--json` em `mostrar` traz `version`, `item`, `planeProject`, `project`, `company`, `tag`, `repo`, `workspace`, `phase`, `source`, `header` e `statePath`.

### Como o header chega ao Claude Code

1. O Claude Code manda em toda chamada os headers de `ANTHROPIC_CUSTOM_HEADERS` (um por linha, `Nome: valor`), lidos do `env` dos arquivos de configuração.
2. Com o comando `etus router setup`, a CLI configura o Claude Code nativamente, registrando hooks em `SessionStart`, `CwdChanged`, `FileChanged` (no `.git/HEAD`) e `UserPromptSubmit`. A cada evento, o hook lê o arquivo de estado e reescreve `ANTHROPIC_CUSTOM_HEADERS` em `.claude/settings.local.json` do projeto com a linha `x-etus-work`, mantendo os outros headers que você já tinha.
3. Além disso, comandos como `etus trabalho <item>` atualizam imediatamente o `.claude/settings.local.json` da pasta atual (garantindo também que o arquivo seja ignorado no `.git/info/exclude`), de modo que as alterações passem a valer a partir do próximo prompt sem reiniciar o Claude Code. O roteador remove o header antes de chamar o provedor.

### Configuração do AI Router

```sh
etus router setup                            # interativo: solicita chave de forma segura
etus router setup <SUA_CHAVE>                # configura diretamente com a chave
etus router setup --model dev-continuo       # define modelo principal e rápido
etus router status                           # inspeciona a configuração atual no Claude Code
etus router                                  # o mesmo que etus router status
```

Nesta CLI, `etus work ...` faz o dispatch para o app Etus Work; por isso o rastreamento de tarefas usa o comando canônico `etus trabalho`.

## Configuração

| O quê | Como |
|---|---|
| Hub | `--hub URL`, `ETUS_HUB_URL` ou `hub_url` no arquivo (padrão `https://apps.etus.io`) |
| Perfil | `--perfil ID` ou `etus perfil usar ID` / `etus perfil limpar`; vai ao gateway como `?perfil=ID` |
| Cache do catálogo | 5 minutos (`cache_ttl_secs` no arquivo); `etus atualizar` renova na hora |
| Arquivo | `config.json` na pasta de configuração do usuário (`etus config` mostra o caminho) |
| Pastas | `ETUS_CONFIG_DIR` troca a pasta inteira |
| Links de ação | `etus config abrir-links sim` ou `nao` (`open_actions` no arquivo), ou `ETUS_OPEN_ACTIONS=1` ou `0`, que vale mais; padrão não abrir sozinho |
| Token | `etus config armazenamento arquivo` ou `chaveiro` (`token_store` no arquivo), ou `ETUS_TOKEN_STORE=file` ou `keychain`, que vale mais; padrão arquivo no macOS e no Linux, Gerenciador de Credenciais no Windows |

Autocompletar: `etus completions zsh > ~/.zfunc/_etus` (também bash, fish, powershell, elvish).

## Desenvolvimento

```sh
cargo test                                  # unitários e integração com um gateway MCP falso
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo test keychain -- --ignored            # testa o chaveiro do sistema de verdade
```

## Próximo passo: assinatura estável

A correção certa para quem quer o chaveiro, e para distribuir o binário, é assinar com um certificado Developer ID (e notarizar). Com uma identidade de código estável entre versões, o macOS reconhece o `etus` atualizado como o mesmo programa e não pede permissão de novo a cada atualização. Hoje o binário é assinado ad hoc (`codesign -dv` mostra `Signature=adhoc`), por isso o arquivo é o padrão. Fica como follow up: assinar e notarizar no fluxo de release e, depois disso, reavaliar o chaveiro como padrão no macOS.

## Publicar uma versão

Sem CI: os binários saem de um Mac com Docker (buildx) e rustup.

1. Suba a versão no `Cargo.toml` (e no `Cargo.lock`) e escreva a seção dela no `CHANGELOG.md`.
2. `scripts/release.sh X.Y.Z` gera em `dist/vX.Y.Z/`:
   - `etus-aarch64-macos` e `etus-x86_64-macos`, nativos, assinados ad hoc (`codesign -s -`);
   - `etus-x86_64-linux` e `etus-aarch64-linux`, estáticos (musl), com `docker buildx build --platform linux/amd64|linux/arm64` e `scripts/release.Dockerfile`;
   - `SHA256SUMS`, `VERSION` e um `<binário>.sha256` por binário.

   No fim, confere o sha256, a assinatura, que os de Linux são estáticos e que cada binário responde `etus X.Y.Z` a `--version` (os de Linux com `docker run --platform`, o Intel com Rosetta).
3. Depois do merge na `main`, `scripts/release.sh X.Y.Z --publish` (com a `main` atualizada e limpa) gera tudo de novo e cria a release `vX.Y.Z` com esses arquivos e as notas do `CHANGELOG.md`. O `etus atualizar-cli` e o instalador procuram exatamente esses nomes.
