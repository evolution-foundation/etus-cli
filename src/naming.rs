pub const BUILTIN_COMMANDS: &[&str] = &[
    "login",
    "logout",
    "whoami",
    "apps",
    "empresa",
    "perfil",
    "atualizar",
    "skills",
    "setup",
    "completions",
    "atualizar-cli",
    "config",
    "help",
];

struct KnownApp {
    prefix: &'static str,
    word: &'static str,
    aliases: &'static [&'static str],
}

const KNOWN_APPS: &[KnownApp] = &[
    KnownApp {
        prefix: "tasks",
        word: "tarefas",
        aliases: &[],
    },
    KnownApp {
        prefix: "records",
        word: "reunioes",
        aliases: &["meetings", "gravador"],
    },
    KnownApp {
        prefix: "muraldereconheci",
        word: "mural",
        aliases: &["recognition"],
    },
    KnownApp {
        prefix: "design",
        word: "design",
        aliases: &["etus-design"],
    },
    KnownApp {
        prefix: "work",
        word: "work",
        aliases: &["etus-work"],
    },
    KnownApp {
        prefix: "extensoes",
        word: "extensoes",
        aliases: &["extensions", "ext"],
    },
    KnownApp {
        prefix: "extensions",
        word: "extensoes",
        aliases: &["ext"],
    },
    KnownApp {
        prefix: "rotinas",
        word: "rotinas",
        aliases: &["routines"],
    },
    KnownApp {
        prefix: "routines",
        word: "rotinas",
        aliases: &[],
    },
    KnownApp {
        prefix: "hub",
        word: "hub",
        aliases: &["plataforma", "platforms"],
    },
];

const BRAND_WORDS: &[&str] = &["etus", "nexus"];

const CONNECTORS: &[&str] = &[
    "de", "da", "do", "das", "dos", "e", "em", "para", "com", "of", "and", "for",
];

const PT_VERBS: &[&str] = &[
    "abrir",
    "aceitar",
    "adicionar",
    "agendar",
    "alterar",
    "apagar",
    "aprovar",
    "arquivar",
    "ativar",
    "atualizar",
    "avaliar",
    "baixar",
    "bloquear",
    "buscar",
    "cadastrar",
    "cancelar",
    "carregar",
    "compartilhar",
    "comentar",
    "concluir",
    "conferir",
    "configurar",
    "consultar",
    "contar",
    "convidar",
    "copiar",
    "criar",
    "dar",
    "definir",
    "deletar",
    "desativar",
    "desenhar",
    "desfazer",
    "desligar",
    "desmarcar",
    "duplicar",
    "editar",
    "enviar",
    "escolher",
    "escrever",
    "excluir",
    "executar",
    "exportar",
    "fechar",
    "fixar",
    "gerar",
    "gravar",
    "importar",
    "iniciar",
    "ler",
    "liberar",
    "ligar",
    "limpar",
    "listar",
    "marcar",
    "mostrar",
    "mover",
    "mudar",
    "obter",
    "parar",
    "pausar",
    "pedir",
    "perguntar",
    "pesquisar",
    "publicar",
    "reabrir",
    "recusar",
    "registrar",
    "rejeitar",
    "remover",
    "renomear",
    "resolver",
    "responder",
    "restaurar",
    "resumir",
    "retomar",
    "revisar",
    "revogar",
    "rodar",
    "salvar",
    "solicitar",
    "subir",
    "sincronizar",
    "transcrever",
    "trocar",
    "usar",
    "validar",
    "ver",
    "verificar",
];

const PT_WORDS: &[&str] = &[
    "ao",
    "aos",
    "arquivo",
    "arquivos",
    "ata",
    "atas",
    "com",
    "comando",
    "comentario",
    "comentarios",
    "conta",
    "da",
    "das",
    "de",
    "dos",
    "empresa",
    "empresas",
    "eu",
    "imagem",
    "imagens",
    "item",
    "itens",
    "lista",
    "meu",
    "meus",
    "minha",
    "minhas",
    "na",
    "nas",
    "nos",
    "nota",
    "notas",
    "ou",
    "para",
    "pela",
    "pelo",
    "pessoa",
    "pessoas",
    "projeto",
    "projetos",
    "quem",
    "sem",
    "seu",
    "seus",
    "sou",
    "sua",
    "suas",
    "tarefa",
    "tarefas",
    "todas",
    "todos",
    "um",
    "uma",
    "umas",
    "uns",
];

pub fn kebab(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    let mut prev: Option<char> = None;
    for c in name.chars() {
        if c == '_' || c == '-' || c == ' ' || c == '.' {
            if !out.is_empty() && !out.ends_with('-') {
                out.push('-');
            }
        } else if c.is_uppercase() {
            if matches!(prev, Some(p) if p.is_lowercase() || p.is_ascii_digit()) && !out.ends_with('-') {
                out.push('-');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
        prev = Some(c);
    }
    out.trim_matches('-').to_string()
}

fn fold_accent(c: char) -> char {
    match c {
        'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'ç' => 'c',
        'ñ' => 'n',
        other => other,
    }
}

pub fn slug(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars().flat_map(char::to_lowercase).map(fold_accent) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn known_app(prefix: &str) -> Option<&'static KnownApp> {
    KNOWN_APPS.iter().find(|k| k.prefix == prefix)
}

pub fn app_word(display_name: &str) -> Option<String> {
    let full = slug(display_name);
    let mut words: Vec<&str> = full.split('-').filter(|w| !w.is_empty()).collect();
    while words.len() > 1 && BRAND_WORDS.contains(&words[0]) {
        words.remove(0);
    }
    if let Some(cut) = words.iter().skip(1).position(|w| CONNECTORS.contains(w)) {
        words.truncate(cut + 1);
    }
    let word = words.join("-");
    (!word.is_empty() && word.len() <= 32).then_some(word)
}

pub fn app_names(prefix: &str, display_name: Option<&str>) -> Vec<String> {
    let known = known_app(prefix);
    let mut names: Vec<String> = Vec::new();
    if let Some(known) = known {
        names.push(known.word.to_string());
    }
    if let Some(word) = display_name.and_then(app_word) {
        names.push(word);
    }
    names.push(prefix.to_string());
    if let Some(known) = known {
        names.extend(known.aliases.iter().map(|a| a.to_string()));
    }
    if let Some(full) = display_name.map(slug).filter(|s| !s.is_empty() && s.len() <= 32) {
        names.push(full);
    }
    let mut unique: Vec<String> = Vec::new();
    for name in names {
        if !unique.contains(&name) {
            unique.push(name);
        }
    }
    unique
}

pub fn is_builtin(word: &str) -> bool {
    BUILTIN_COMMANDS.contains(&word)
}

pub fn has_accent(text: &str) -> bool {
    text.chars().flat_map(char::to_lowercase).any(|c| fold_accent(c) != c)
}

pub fn is_pt_br(title: &str) -> bool {
    if has_accent(title) {
        return true;
    }
    let folded = slug(title);
    let words: Vec<&str> = folded.split('-').filter(|w| !w.is_empty()).collect();
    match words.first() {
        Some(first) if PT_VERBS.contains(first) => true,
        _ => words.iter().any(|w| PT_WORDS.contains(w)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_become_kebab_commands() {
        assert_eq!(kebab("agenda_listar"), "agenda-listar");
        assert_eq!(kebab("list_tasks"), "list-tasks");
        assert_eq!(kebab("projectId"), "project-id");
        assert_eq!(kebab("dueDate"), "due-date");
        assert_eq!(kebab("send_todo_to_tasks"), "send-todo-to-tasks");
        assert_eq!(kebab("item2Name"), "item2-name");
        assert_eq!(kebab("already-kebab"), "already-kebab");
        assert_eq!(kebab("__weird__"), "weird");
    }

    #[test]
    fn slugs_drop_accents() {
        assert_eq!(slug("Reuniões"), "reunioes");
        assert_eq!(slug("Listar tarefas"), "listar-tarefas");
        assert_eq!(slug("Trocar a empresa ativa"), "trocar-a-empresa-ativa");
        assert_eq!(slug("ETUS Platforms"), "etus-platforms");
        assert_eq!(slug("Extensões"), "extensoes");
    }

    #[test]
    fn app_words_come_from_the_map_then_the_name() {
        assert_eq!(app_names("tasks", Some("Tarefas")), vec!["tarefas", "tasks"]);
        assert_eq!(
            app_names("records", Some("Reuniões")),
            vec!["reunioes", "records", "meetings", "gravador"]
        );
        assert_eq!(
            app_names("muraldereconheci", Some("Mural de Reconhecimento")),
            vec!["mural", "muraldereconheci", "recognition", "mural-de-reconhecimento"]
        );
        assert_eq!(app_names("design", Some("Etus Design")), vec!["design", "etus-design"]);
        assert_eq!(app_names("work", Some("Etus Work")), vec!["work", "etus-work"]);
        assert_eq!(
            app_names("extensoes", Some("Extensões de IA")),
            vec!["extensoes", "extensions", "ext", "extensoes-de-ia"]
        );
        assert_eq!(app_names("rotinas", Some("Rotinas")), vec!["rotinas", "routines"]);
        assert_eq!(
            app_names("hub", Some("ETUS Platforms")),
            vec!["hub", "platforms", "plataforma", "etus-platforms"]
        );
        assert_eq!(app_names("crm", None), vec!["crm"]);
        assert_eq!(
            app_names("fin", Some("Gestão Financeira")),
            vec!["gestao-financeira", "fin"]
        );
        assert_eq!(
            app_names("cal", Some("Agenda de Salas")),
            vec!["agenda", "cal", "agenda-de-salas"]
        );
    }

    #[test]
    fn detects_pt_br_titles() {
        assert!(is_pt_br("Criar tarefa"));
        assert!(is_pt_br("Listar tarefas"));
        assert!(is_pt_br("Reuniões recentes"));
        assert!(is_pt_br("Quem sou eu"));
        assert!(is_pt_br("Ver projeto"));
        for title in [
            "Minhas tarefas",
            "Visão geral das tarefas",
            "Perguntar às reuniões",
            "Ver ata",
            "Cadastrar app",
        ] {
            assert!(is_pt_br(title), "{title}");
        }
        assert!(!is_pt_br("Create task"));
        assert!(!is_pt_br("List tasks"));
        assert!(!is_pt_br("Get design system"));
        assert!(!is_pt_br("Do it now"));
    }
}
