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

const STATIC_APP_ALIASES: &[(&str, &[&str])] = &[
    ("records", &["reunioes", "gravador"]),
    ("tasks", &["tarefas"]),
    ("design", &["etus-design"]),
    ("work", &["etus-work"]),
    ("routines", &["rotinas"]),
    ("rotinas", &["routines"]),
    ("extensoes", &["extensions", "ext"]),
    ("extensions", &["extensoes"]),
    ("hub", &["plataforma"]),
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

pub fn app_aliases(prefix: &str, display_name: Option<&str>) -> Vec<String> {
    let mut aliases: Vec<String> = STATIC_APP_ALIASES
        .iter()
        .filter(|(p, _)| *p == prefix)
        .flat_map(|(_, list)| list.iter().map(|a| a.to_string()))
        .collect();
    if let Some(name) = display_name {
        let derived = slug(name);
        if !derived.is_empty() && derived.len() <= 32 {
            aliases.push(derived);
        }
    }
    let mut unique: Vec<String> = Vec::new();
    for alias in aliases {
        if alias != prefix && !BUILTIN_COMMANDS.contains(&alias.as_str()) && !unique.contains(&alias) {
            unique.push(alias);
        }
    }
    unique
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
    fn app_aliases_mix_static_map_and_display_name() {
        assert_eq!(app_aliases("tasks", Some("Tarefas")), vec!["tarefas"]);
        assert_eq!(app_aliases("records", Some("Reuniões")), vec!["reunioes", "gravador"]);
        assert_eq!(app_aliases("design", Some("Etus Design")), vec!["etus-design"]);
        assert_eq!(app_aliases("crm", None), Vec::<String>::new());
        assert_eq!(app_aliases("crm", Some("CRM")), Vec::<String>::new());
        assert_eq!(app_aliases("x", Some("Apps")), Vec::<String>::new());
    }
}
