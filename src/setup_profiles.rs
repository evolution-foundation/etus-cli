use std::collections::BTreeSet;
use std::path::Path;

use serde::Deserialize;

use crate::error::{Error, Result};

const EMBEDDED: &str = include_str!("setup-profiles.json");
pub const PROFILES_ENV: &str = "ETUS_SKILLS_PROFILES";
pub const MARKETPLACE: &str = "etus";
pub const REPOSITORY: &str = "evolution-foundation/etus-skills";
const OPTIONAL_PACKS: [(&str, &[&str]); 1] = [("design", &["etus-design-templates"])];
const DETECTION: [(&str, &[&str], &[&str]); 5] = [
    ("app-nexus", &["guard.config.json", "lib/api-spec/openapi.yaml"], &[]),
    (
        "codigo",
        &["package.json", "go.mod", "Cargo.toml", "pyproject.toml"],
        &[],
    ),
    ("produto", &[], &["docs/produto", "prds"]),
    ("design", &[], &["design-systems", "components/ui"]),
    ("ops", &["router.config.json", "docker-compose.router.yml"], &[]),
];

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Profile {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub packs: Vec<String>,
}

impl Profile {
    fn optional(&self) -> &'static [&'static str] {
        OPTIONAL_PACKS
            .iter()
            .find(|(id, _)| *id == self.id)
            .map_or(&[], |(_, packs)| packs)
    }

    pub fn plugins(&self, with_optional: bool) -> Vec<String> {
        let optional = self.optional();
        self.packs
            .iter()
            .filter(|pack| with_optional || !optional.contains(&pack.as_str()))
            .map(|pack| format!("{pack}@{MARKETPLACE}"))
            .collect()
    }

    fn detected(&self, root: &Path) -> bool {
        let Some((_, files, dirs)) = DETECTION.iter().find(|(id, _, _)| *id == self.id) else {
            return false;
        };
        files.iter().any(|f| root.join(f).is_file()) || dirs.iter().any(|d| root.join(d).is_dir())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Profiles {
    pub list: Vec<Profile>,
}

#[derive(Deserialize)]
struct Document {
    profiles: Vec<Profile>,
}

impl Profiles {
    pub fn parse(text: &str) -> Result<Profiles> {
        let doc: Document =
            serde_json::from_str(text).map_err(|e| Error::Usage(format!("Lista de perfis inválida: {e}")))?;
        if doc.profiles.is_empty() {
            return Err(Error::Usage("A lista de perfis está vazia.".into()));
        }
        Ok(Profiles { list: doc.profiles })
    }

    pub fn load() -> Result<Profiles> {
        match std::env::var_os(PROFILES_ENV).filter(|v| !v.is_empty()) {
            Some(path) => {
                let text = std::fs::read_to_string(&path).map_err(|e| {
                    Error::Usage(format!(
                        "{PROFILES_ENV} aponta para {}, que não deu para ler: {e}",
                        Path::new(&path).display()
                    ))
                })?;
                Profiles::parse(&text)
            }
            None => Profiles::parse(EMBEDDED),
        }
    }

    pub fn ids(&self) -> Vec<&str> {
        self.list.iter().map(|p| p.id.as_str()).collect()
    }

    pub fn get(&self, id: &str) -> Option<&Profile> {
        self.list.iter().find(|p| p.id == id)
    }

    pub fn check(&self, ids: &[String], place: &str) -> Result<()> {
        let unknown: Vec<&str> = ids
            .iter()
            .filter(|id| self.get(id).is_none())
            .map(String::as_str)
            .collect();
        if unknown.is_empty() {
            return Ok(());
        }
        Err(Error::Usage(format!(
            "Perfil desconhecido{place}: {}. Perfis disponíveis: {}",
            unknown.join(", "),
            self.ids().join(", ")
        )))
    }

    pub fn detect(&self, root: &Path) -> Vec<String> {
        let found: Vec<&Profile> = self.list.iter().filter(|p| p.detected(root)).collect();
        found
            .iter()
            .filter(|p| {
                !found.iter().any(|other| {
                    other.id != p.id
                        && other.packs.len() > p.packs.len()
                        && p.packs.iter().all(|pack| other.packs.contains(pack))
                })
            })
            .map(|p| p.id.clone())
            .collect()
    }

    pub fn plugins(&self, ids: &[String], with_optional: bool) -> Vec<String> {
        let mut seen = BTreeSet::new();
        ids.iter()
            .filter_map(|id| self.get(id))
            .flat_map(|p| p.plugins(with_optional))
            .filter(|plugin| seen.insert(plugin.clone()))
            .collect()
    }
}

pub fn split_ids(values: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in values.iter().flat_map(|v| v.split(',')).map(str::trim) {
        if !id.is_empty() && !out.iter().any(|o| o == id) {
            out.push(id.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    #[test]
    fn embedded_profiles_match_the_etus_skills_format() {
        let profiles = Profiles::parse(EMBEDDED).unwrap();
        assert_eq!(
            profiles.ids(),
            vec!["app-nexus", "codigo", "produto", "design", "ops", "admin"]
        );
        for profile in &profiles.list {
            assert!(profile.packs.contains(&"etus-core".to_string()), "{}", profile.id);
        }
    }

    #[test]
    fn plugins_follow_the_profiles_and_the_optional_packs() {
        let profiles = Profiles::parse(EMBEDDED).unwrap();
        let ids = vec!["codigo".to_string(), "design".to_string()];
        assert_eq!(
            profiles.plugins(&ids, false),
            vec!["etus-core@etus", "etus-dev@etus", "etus-design@etus"]
        );
        assert!(
            profiles
                .plugins(&ids, true)
                .contains(&"etus-design-templates@etus".to_string())
        );
        let err = profiles.check(&["nada".into()], " no .etus.json").unwrap_err();
        assert!(err.message().contains("nada"));
    }

    #[test]
    fn detection_drops_profiles_covered_by_a_larger_one() {
        let profiles = Profiles::parse(EMBEDDED).unwrap();
        let root = temp_dir("profiles-detect");
        assert!(profiles.detect(&root).is_empty());
        std::fs::write(root.join("package.json"), "{}").unwrap();
        assert_eq!(profiles.detect(&root), vec!["codigo"]);
        std::fs::write(root.join("guard.config.json"), "{}").unwrap();
        assert_eq!(profiles.detect(&root), vec!["app-nexus"]);
    }

    #[test]
    fn ids_split_on_commas_without_repeats() {
        let ids = split_ids(&["codigo, design".into(), "codigo".into(), " ".into()]);
        assert_eq!(ids, vec!["codigo", "design"]);
    }
}
