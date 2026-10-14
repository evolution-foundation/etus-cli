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

O token fica no chaveiro do sistema (macOS e Windows) ou em um arquivo com permissão 0600 na pasta de configuração (Linux, ou quando o chaveiro falha). Nada de token aparece em log ou saída.

## Usar

```sh
etus tarefas --help                                  # comandos do app (alias de tasks)
etus tasks list-tasks --help                         # descrição e parâmetros
etus tasks list-tasks --view today --today 2026-10-14
etus tasks create-task --title "Revisar contrato" --due-date 2026-10-20 --yes
etus reunioes search-meetings --q "preço" --json
etus empresa listar
etus empresa usar etus                               # id, slug ou nome
etus skills buscar "planilha de custos"
etus skills ler <id>
```

Nomes: a ferramenta `tasks__create_task` vira `etus tasks create-task`; o título em pt-BR ("Criar tarefa") vira o alias `criar-tarefa`. Apps têm aliases pelo nome (`tarefas`, `reunioes`, `rotinas`, `extensoes`...).

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
| Pastas | `ETUS_CONFIG_DIR` troca a pasta inteira; `ETUS_TOKEN_STORE=file` força o arquivo em vez do chaveiro |

Autocompletar: `etus completions zsh > ~/.zfunc/_etus` (também bash, fish, powershell, elvish).

## Desenvolvimento

```sh
cargo test                                  # unitários e integração com um gateway MCP falso
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo test keychain -- --ignored            # testa o chaveiro do macOS de verdade
```

## Publicar uma versão

1. Suba a versão no `Cargo.toml` e no `CHANGELOG.md`.
2. Gere os binários:
   ```sh
   cargo build --release --target aarch64-apple-darwin
   cargo build --release --target x86_64-apple-darwin
   cargo zigbuild --release --target x86_64-unknown-linux-gnu   # se tiver cargo-zigbuild
   ```
3. Renomeie para `etus-<arch>-<os>` (`etus-aarch64-macos`, `etus-x86_64-macos`, `etus-x86_64-linux`), gere `<nome>.sha256` com `shasum -a 256` e crie a release com a tag `vX.Y.Z` anexando os binários e os `.sha256`. O `etus atualizar-cli` procura exatamente esses nomes e confere o sha256 quando existe.
