# Changelog

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
