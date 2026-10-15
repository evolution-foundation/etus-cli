use clap::{Arg, ArgAction, ArgMatches};
use serde_json::{Map, Number, Value};

use crate::error::{Error, Result};
use crate::naming::kebab;

pub const RESERVED_FLAGS: &[&str] = &[
    "help", "json", "input", "yes", "dry-run", "quiet", "hub", "perfil", "version", "open", "abrir", "wait", "aguardar",
];

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    String,
    Integer,
    Number,
    Boolean,
    Array(Box<Kind>),
    Json,
}

impl Kind {
    pub fn label(&self) -> String {
        match self {
            Kind::String => "texto".into(),
            Kind::Integer => "inteiro".into(),
            Kind::Number => "número".into(),
            Kind::Boolean => "sim/não".into(),
            Kind::Array(item) => format!("lista de {}", item.label()),
            Kind::Json => "JSON".into(),
        }
    }

    fn value_name(&self) -> &'static str {
        match self {
            Kind::String => "TEXTO",
            Kind::Integer | Kind::Number => "NÚMERO",
            Kind::Boolean => "BOOL",
            Kind::Array(_) => "VALOR",
            Kind::Json => "JSON",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub flag: String,
    pub kind: Kind,
    pub required: bool,
    pub nullable: bool,
    pub description: Option<String>,
    pub enum_values: Vec<Value>,
    pub default: Option<Value>,
    pub format: Option<String>,
}

fn kind_from_type(name: &str, schema: &Value) -> Kind {
    match name {
        "string" => Kind::String,
        "integer" => Kind::Integer,
        "number" => Kind::Number,
        "boolean" => Kind::Boolean,
        "array" => {
            let item = schema.get("items").map(|items| kind_of(items).0).unwrap_or(Kind::Json);
            match item {
                Kind::Array(_) => Kind::Array(Box::new(Kind::Json)),
                other => Kind::Array(Box::new(other)),
            }
        }
        _ => Kind::Json,
    }
}

fn kind_of(schema: &Value) -> (Kind, bool) {
    match schema.get("type") {
        Some(Value::String(t)) => return (kind_from_type(t, schema), t == "null"),
        Some(Value::Array(types)) => {
            let names: Vec<&str> = types.iter().filter_map(Value::as_str).collect();
            let nullable = names.contains(&"null");
            let concrete: Vec<&str> = names.into_iter().filter(|t| *t != "null").collect();
            let kind = match concrete.as_slice() {
                [single] => kind_from_type(single, schema),
                _ => Kind::Json,
            };
            return (kind, nullable);
        }
        _ => {}
    }
    for key in ["anyOf", "oneOf"] {
        if let Some(Value::Array(options)) = schema.get(key) {
            let is_null = |o: &Value| o.get("type").and_then(Value::as_str) == Some("null");
            let nullable = options.iter().any(is_null);
            let concrete: Vec<&Value> = options.iter().filter(|o| !is_null(o)).collect();
            let kind = match concrete.as_slice() {
                [single] => kind_of(single).0,
                many if !many.is_empty() && many.iter().all(|o| o.get("const").is_some_and(Value::is_string)) => {
                    Kind::String
                }
                _ => Kind::Json,
            };
            return (kind, nullable);
        }
    }
    if let Some(Value::Array(values)) = schema.get("enum") {
        let nullable = values.iter().any(Value::is_null);
        let kind = if values.iter().filter(|v| !v.is_null()).all(Value::is_string) {
            Kind::String
        } else {
            Kind::Json
        };
        return (kind, nullable);
    }
    if schema.get("const").is_some_and(Value::is_string) {
        return (Kind::String, false);
    }
    (Kind::Json, false)
}

fn enum_values(schema: &Value) -> Vec<Value> {
    if let Some(Value::Array(values)) = schema.get("enum") {
        return values.clone();
    }
    for key in ["anyOf", "oneOf"] {
        if let Some(Value::Array(options)) = schema.get(key) {
            let mut values = Vec::new();
            for option in options {
                if let Some(Value::Array(inner)) = option.get("enum") {
                    values.extend(inner.iter().cloned());
                } else if let Some(c) = option.get("const") {
                    values.push(c.clone());
                }
            }
            if !values.is_empty() {
                return values;
            }
        }
    }
    schema.get("const").map(|c| vec![c.clone()]).unwrap_or_default()
}

pub fn params(schema: &Value) -> Vec<Param> {
    let Some(Value::Object(properties)) = schema.get("properties") else {
        return Vec::new();
    };
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut taken: Vec<String> = RESERVED_FLAGS.iter().map(|s| s.to_string()).collect();
    let mut out = Vec::with_capacity(properties.len());
    for (name, prop) in properties {
        let (kind, nullable) = kind_of(prop);
        let mut flag = kebab(name);
        if flag.is_empty() || taken.contains(&flag) {
            flag = format!("arg-{}", if flag.is_empty() { "x".into() } else { flag });
        }
        let mut n = 2;
        let base = flag.clone();
        while taken.contains(&flag) {
            flag = format!("{base}-{n}");
            n += 1;
        }
        taken.push(flag.clone());
        out.push(Param {
            name: name.clone(),
            flag,
            kind,
            required: required.contains(&name.as_str()),
            nullable,
            description: prop.get("description").and_then(Value::as_str).map(str::to_string),
            enum_values: enum_values(prop),
            default: prop.get("default").cloned(),
            format: prop.get("format").and_then(Value::as_str).map(str::to_string),
        });
    }
    out.sort_by_key(|p| !p.required);
    out
}

fn display(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

pub fn param_help(param: &Param) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(description) = &param.description {
        parts.push(description.trim().to_string());
    }
    let mut facts = vec![param.kind.label()];
    if param.required {
        facts.insert(0, "obrigatório".into());
    }
    if let Some(format) = &param.format {
        facts.push(format!("formato {format}"));
    }
    if !param.enum_values.is_empty() {
        let values: Vec<String> = param.enum_values.iter().map(display).collect();
        if values.join("|").len() > 40 || matches!(param.kind, Kind::Array(_) | Kind::Boolean) {
            facts.push(format!("valores: {}", values.join(", ")));
        }
    }
    if let Some(default) = &param.default {
        facts.push(format!("padrão: {}", display(default)));
    }
    if param.nullable {
        facts.push("null apaga".into());
    }
    if matches!(param.kind, Kind::Array(_)) {
        facts.push("repita a flag ou passe um array JSON".into());
    }
    parts.push(format!("[{}]", facts.join("; ")));
    parts.join(" ")
}

fn value_name(param: &Param, kind: &Kind) -> String {
    let choices: Vec<String> = param.enum_values.iter().map(display).collect();
    let joined = choices.join("|");
    if !choices.is_empty() && joined.len() <= 40 {
        joined
    } else {
        kind.value_name().to_string()
    }
}

pub fn to_args(params: &[Param]) -> Vec<Arg> {
    params
        .iter()
        .map(|param| {
            let arg = Arg::new(param.flag.clone())
                .long(param.flag.clone())
                .help(param_help(param))
                .help_heading("Parâmetros");
            match &param.kind {
                Kind::Boolean => arg
                    .num_args(0..=1)
                    .default_missing_value("true")
                    .value_name("true|false")
                    .action(ArgAction::Set),
                Kind::Array(_) => arg.value_name(param.kind.value_name()).action(ArgAction::Append),
                kind => arg.value_name(value_name(param, kind)).action(ArgAction::Set),
            }
        })
        .collect()
}

fn usage(param: &Param, expected: &str) -> Error {
    Error::Usage(format!("--{} espera {expected}.", param.flag))
}

pub fn convert(param: &Param, kind: &Kind, raw: &str) -> Result<Value> {
    if param.nullable && raw == "null" {
        return Ok(Value::Null);
    }
    match kind {
        Kind::String => Ok(Value::String(raw.to_string())),
        Kind::Integer => raw
            .trim()
            .parse::<i64>()
            .map(Value::from)
            .map_err(|_| usage(param, "um número inteiro")),
        Kind::Number => {
            let trimmed = raw.trim();
            if let Ok(i) = trimmed.parse::<i64>() {
                return Ok(Value::from(i));
            }
            trimmed
                .parse::<f64>()
                .ok()
                .and_then(Number::from_f64)
                .map(Value::Number)
                .ok_or_else(|| usage(param, "um número"))
        }
        Kind::Boolean => match raw.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "sim" | "s" | "yes" => Ok(Value::Bool(true)),
            "false" | "0" | "nao" | "não" | "n" | "no" => Ok(Value::Bool(false)),
            _ => Err(usage(param, "true ou false")),
        },
        Kind::Array(item) => {
            let trimmed = raw.trim();
            if trimmed.starts_with('[') {
                return serde_json::from_str(trimmed).map_err(|_| usage(param, "um array JSON válido"));
            }
            Ok(Value::Array(vec![convert(param, item, raw)?]))
        }
        Kind::Json => Ok(serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()))),
    }
}

pub fn collect(params: &[Param], matches: &ArgMatches) -> Result<Map<String, Value>> {
    let mut out = Map::new();
    for param in params {
        let Some(values) = matches.get_many::<String>(&param.flag) else {
            continue;
        };
        let values: Vec<&String> = values.collect();
        let value = match &param.kind {
            Kind::Array(item) => {
                if values.len() == 1 {
                    convert(param, &param.kind, values[0])?
                } else {
                    let items = values
                        .iter()
                        .map(|raw| convert(param, item, raw))
                        .collect::<Result<Vec<_>>>()?;
                    Value::Array(items)
                }
            }
            kind => convert(param, kind, values[values.len() - 1])?,
        };
        out.insert(param.name.clone(), value);
    }
    Ok(out)
}

pub fn merge(base: Option<Value>, flags: Map<String, Value>) -> Result<Map<String, Value>> {
    let mut merged = match base {
        None => Map::new(),
        Some(Value::Object(map)) => map,
        Some(_) => return Err(Error::Usage("Os argumentos em JSON precisam ser um objeto.".into())),
    };
    merged.extend(flags);
    Ok(merged)
}

pub fn validate(params: &[Param], args: &Map<String, Value>) -> Result<()> {
    let missing: Vec<String> = params
        .iter()
        .filter(|p| p.required && !args.contains_key(&p.name))
        .map(|p| format!("--{}", p.flag))
        .collect();
    if !missing.is_empty() {
        return Err(Error::Usage(format!(
            "Faltam parâmetros obrigatórios: {}.",
            missing.join(", ")
        )));
    }
    for param in params {
        if param.enum_values.is_empty() {
            continue;
        }
        let Some(value) = args.get(&param.name) else { continue };
        if value.is_null() && param.nullable {
            continue;
        }
        let candidates: Vec<&Value> = match value {
            Value::Array(items) if matches!(param.kind, Kind::Array(_)) => items.iter().collect(),
            single => vec![single],
        };
        for candidate in candidates {
            if !param.enum_values.contains(candidate) {
                let allowed: Vec<String> = param.enum_values.iter().map(display).collect();
                return Err(Error::Usage(format!(
                    "--{} não aceita {}. Valores: {}.",
                    param.flag,
                    display(candidate),
                    allowed.join(", ")
                )));
            }
        }
    }
    Ok(())
}

pub fn example(params: &[Param]) -> String {
    params
        .iter()
        .filter(|p| p.required)
        .map(|p| {
            let value = p.enum_values.first().map(display).unwrap_or_else(|| match p.kind {
                Kind::Integer | Kind::Number => "1".into(),
                Kind::Boolean => "true".into(),
                Kind::Json => "'{}'".into(),
                _ => format!("<{}>", p.flag),
            });
            format!("--{} {}", p.flag, value)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Command;
    use serde_json::json;

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "Task title"},
                "priority": {"anyOf": [{"const": 1, "type": "number"}, {"const": 2, "type": "number"}], "description": "1 is urgent"},
                "dueDate": {"type": ["string", "null"], "format": "date"},
                "done": {"type": "boolean"},
                "tags": {"type": "array", "items": {"type": "string"}},
                "ids": {"type": "array", "items": {"type": "integer"}},
                "status": {"type": "string", "enum": ["open", "done", "all"], "default": "open"},
                "limit": {"type": "integer", "minimum": 1},
                "json": {"type": "string"},
                "filter": {"type": "object"}
            },
            "required": ["title", "status"]
        })
    }

    fn parse(args: &[&str]) -> Result<Map<String, Value>> {
        let params = params(&schema());
        let cmd = Command::new("t").args(to_args(&params));
        let mut argv = vec!["t"];
        argv.extend_from_slice(args);
        let matches = cmd
            .try_get_matches_from(argv)
            .map_err(|e| Error::Usage(e.to_string()))?;
        collect(&params, &matches)
    }

    #[test]
    fn schema_maps_to_flags() {
        let params = params(&schema());
        let find = |name: &str| params.iter().find(|p| p.name == name).unwrap().clone();
        assert!(params[0].required && params[1].required);
        assert_eq!(find("dueDate").flag, "due-date");
        assert_eq!(find("dueDate").kind, Kind::String);
        assert!(find("dueDate").nullable);
        assert_eq!(find("priority").kind, Kind::Json);
        assert_eq!(find("priority").enum_values, vec![json!(1), json!(2)]);
        assert_eq!(find("tags").kind, Kind::Array(Box::new(Kind::String)));
        assert_eq!(find("ids").kind, Kind::Array(Box::new(Kind::Integer)));
        assert_eq!(find("status").default, Some(json!("open")));
        assert_eq!(find("json").flag, "arg-json");
        assert_eq!(find("filter").kind, Kind::Json);
        let help = param_help(&find("status"));
        assert!(help.contains("obrigatório") && help.contains("padrão: open"));
        let args = to_args(&params);
        let status = args.iter().find(|a| a.get_id() == "status").unwrap();
        assert_eq!(status.get_value_names().unwrap()[0], "open|done|all");
    }

    #[test]
    fn flags_become_json_arguments() {
        let args = parse(&[
            "--title",
            "Oi",
            "--status",
            "done",
            "--due-date",
            "null",
            "--done",
            "--tags",
            "a",
            "--tags",
            "b",
            "--ids",
            "[1,2]",
            "--limit",
            "5",
            "--priority",
            "2",
            "--arg-json",
            "x",
            "--filter",
            "{\"a\":1}",
        ])
        .unwrap();
        assert_eq!(
            Value::Object(args),
            json!({
                "title": "Oi", "status": "done", "dueDate": null, "done": true, "tags": ["a", "b"],
                "ids": [1, 2], "limit": 5, "priority": 2, "json": "x", "filter": {"a": 1}
            })
        );
        let args = parse(&["--title", "x", "--done", "false", "--ids", "7"]).unwrap();
        assert_eq!(args.get("done"), Some(&json!(false)));
        assert_eq!(args.get("ids"), Some(&json!([7])));
    }

    #[test]
    fn bad_values_are_usage_errors() {
        assert!(matches!(parse(&["--limit", "abc"]), Err(Error::Usage(_))));
        assert!(matches!(parse(&["--done", "talvez"]), Err(Error::Usage(_))));
        assert!(matches!(parse(&["--ids", "[1,"]), Err(Error::Usage(_))));
    }

    #[test]
    fn required_and_enums_are_checked_locally() {
        let params = params(&schema());
        let missing = validate(&params, &Map::new()).unwrap_err();
        assert_eq!(missing.exit_code(), 2);
        assert!(missing.message().contains("--title") && missing.message().contains("--status"));
        let wrong = merge(Some(json!({"title": "a", "status": "nope"})), Map::new()).unwrap();
        assert!(
            validate(&params, &wrong)
                .unwrap_err()
                .message()
                .contains("open, done, all")
        );
        let ok = merge(Some(json!({"title": "a", "status": "all", "priority": 1})), Map::new()).unwrap();
        assert!(validate(&params, &ok).is_ok());
        let bad_priority = merge(Some(json!({"title": "a", "status": "all", "priority": 3})), Map::new()).unwrap();
        assert!(validate(&params, &bad_priority).is_err());
    }

    #[test]
    fn flags_override_json_input() {
        let mut flags = Map::new();
        flags.insert("title".into(), json!("flag"));
        let merged = merge(Some(json!({"title": "json", "status": "open"})), flags).unwrap();
        assert_eq!(Value::Object(merged), json!({"title": "flag", "status": "open"}));
        assert!(merge(Some(json!([1])), Map::new()).is_err());
    }

    #[test]
    fn example_uses_required_params() {
        assert_eq!(example(&params(&schema())), "--title <title> --status open");
    }
}
