# Changelog

## 0.1.0 (2026-10-14)

Primeira versão.

- `etus login` pelo navegador com OAuth 2.1 do hub: registro dinâmico do cliente (guardado por hub), PKCE S256, retorno em `http://127.0.0.1:<porta>/callback`, opção `--no-browser` com colar o endereço de retorno e `--somente-leitura`.
- Token no chaveiro do sistema (macOS e Windows) ou em arquivo 0600; refresh com rotação feito sob trava de arquivo, para dois processos não queimarem o mesmo refresh token.
- Comandos gerados do `tools/list` do gateway `https://apps.etus.io/mcp`: um grupo por prefixo de app, comandos em kebab-case, aliases em pt-BR pelo título da ferramenta e pelo nome do app, flags a partir do JSON Schema, `--json` / `--input` para o objeto inteiro, validação local de obrigatórios e valores.
- Confirmação para ferramentas que alteram dados (`--yes` ou pergunta no terminal) e `--dry-run`.
- Saída legível por padrão, `--json` compacto e `--quiet`; códigos de saída 0, 1, 2, 3 e 4.
- `etus whoami`, `etus apps`, `etus empresa listar|usar`, `etus perfil mostrar|usar|limpar` (passa `?perfil=` ao gateway), `etus atualizar`, `etus skills buscar|ler`, `etus setup` (em breve), `etus completions`, `etus config` e `etus atualizar-cli` (releases do GitHub, com o token do `gh`).
- Cache do catálogo por hub, perfil e pessoa, com 5 minutos de validade.
