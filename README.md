# etus

CLI global da plataforma ETUS. Um binário só, `etus`, gera os comandos de **todos os apps** a partir das ferramentas MCP que o hub libera para você: um app novo com ferramentas MCP vira `etus <app> <comando>` sem nenhuma mudança aqui.

Para agentes de código (Claude Code, Codex) é a interface mais econômica: o agente só lê `--help` quando precisa e usa `--json` para receber a resposta compacta.

Contrato completo: `docs/specs/cli.md` no repositório do hub (`etusdigital/nexus-platforms`).

## Instalar

O repositório é privado. Com o GitHub CLI autenticado:

```sh
# macOS Apple Silicon
gh release download --repo evolution-foundation/etus-cli --pattern etus-aarch64-macos --output etus
# macOS Intel: etus-x86_64-macos | Linux: etus-x86_64-linux
chmod +x etus && sudo mv etus /usr/local/bin/etus
etus --version
```

Ou compile (Rust 1.89 ou mais novo):

```sh
cargo install --git ssh://git@github.com/evolution-foundation/etus-cli --locked
```

Depois, `etus atualizar-cli` baixa a versão mais nova sozinho (usa `GH_TOKEN`, `GITHUB_TOKEN` ou `gh auth token`).

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

## Configuração

| O quê | Como |
|---|---|
| Hub | `--hub URL`, `ETUS_HUB_URL` ou `hub_url` no arquivo (padrão `https://apps.etus.io`) |
| Perfil | `--perfil ID` ou `etus perfil usar ID` / `etus perfil limpar`; vai ao gateway como `?perfil=ID` |
| Cache do catálogo | 5 minutos (`cache_ttl_secs` no arquivo); `etus atualizar` renova na hora |
| Arquivo | `config.json` na pasta de configuração do usuário (`etus config` mostra o caminho) |
| Pastas | `ETUS_CONFIG_DIR` troca a pasta inteira |
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

1. Suba a versão no `Cargo.toml` e no `CHANGELOG.md`.
2. Gere os binários:
   ```sh
   cargo build --release --target aarch64-apple-darwin
   TC=~/.rustup/toolchains/stable-aarch64-apple-darwin/bin
   RUSTC=$TC/rustc $TC/cargo build --release --target x86_64-apple-darwin   # o cargo do Homebrew não tem a std do x86_64
   cargo zigbuild --release --target x86_64-unknown-linux-gnu   # se tiver cargo-zigbuild
   ```
3. Renomeie para `etus-<arch>-<os>` (`etus-aarch64-macos`, `etus-x86_64-macos`, `etus-x86_64-linux`), gere `<nome>.sha256` com `shasum -a 256` e crie a release com a tag `vX.Y.Z` anexando os binários e os `.sha256`. O `etus atualizar-cli` procura exatamente esses nomes e confere o sha256 quando existe.
