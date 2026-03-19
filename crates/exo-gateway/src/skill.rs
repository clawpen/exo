use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Skill manifest defines a skill's capabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillManifest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: Option<String>,
    pub runtime: SkillRuntime,
    pub tools: Vec<ToolDef>,
    pub config: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillRuntime {
    Container {
        image: String,
        #[serde(default)]
        resources: ContainerResources,
        #[serde(default)]
        env: HashMap<String, String>,
    },
    Wasm {
        module: String,
        #[serde(default)]
        memory_limit_mb: u32,
    },
    Builtin,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ContainerResources {
    #[serde(default = "default_memory")]
    pub memory: String,
    #[serde(default = "default_cpu")]
    pub cpu: f32,
    #[serde(default)]
    pub gpu: bool,
}

fn default_memory() -> String {
    "512M".to_string()
}

fn default_cpu() -> f32 {
    0.5
}

/// Tool definition within a skill
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value, // JSON Schema
    pub returns: Option<serde_json::Value>, // JSON Schema
    pub timeout_ms: Option<u64>,
}

/// Registered skill instance
#[derive(Debug, Clone)]
pub struct Skill {
    pub manifest: SkillManifest,
    pub source_path: Option<std::path::PathBuf>,
}

/// Manages available skills and their tools
#[derive(Debug, Clone)]
pub struct SkillRegistry {
    skills: Arc<RwLock<HashMap<String, Skill>>>,
    skills_dir: Option<std::path::PathBuf>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self {
            skills: Arc::new(RwLock::new(HashMap::new())),
            skills_dir: None,
        }
    }
    
    pub fn with_skills_dir(skills_dir: impl AsRef<Path>) -> Self {
        Self {
            skills: Arc::new(RwLock::new(HashMap::new())),
            skills_dir: Some(skills_dir.as_ref().to_path_buf()),
        }
    }
    
    /// Register a skill from a manifest
    pub async fn register(&self, manifest: SkillManifest, source_path: Option<std::path::PathBuf>) -> Result<(), SkillError> {
        let name = manifest.name.clone();
        
        // Validate: check for duplicate tools
        let existing = self.skills.read().await;
        for skill in existing.values() {
            for tool in &manifest.tools {
                if skill.manifest.tools.iter().any(|t| t.name == tool.name) {
                    return Err(SkillError::DuplicateTool(tool.name.clone()));
                }
            }
        }
        drop(existing);
        
        let skill = Skill { manifest, source_path };
        self.skills.write().await.insert(name.clone(), skill);
        info!(skill = %name, "Skill registered");
        Ok(())
    }
    
    /// Load skills from directory
    pub async fn load_from_dir(&self, dir: impl AsRef<Path>) -> Result<usize, SkillError> {
        let dir = dir.as_ref();
        let mut count = 0;
        
        let mut entries = tokio::fs::read_dir(dir).await
            .map_err(|e| SkillError::Io(e.to_string()))?;
        
        while let Some(entry) = entries.next_entry().await
            .map_err(|e| SkillError::Io(e.to_string()))? {
            
            let path = entry.path();
            if path.is_dir() {
                let manifest_path = path.join("skill.yaml");
                if manifest_path.exists() {
                    match self.load_manifest(&manifest_path).await {
                        Ok(manifest) => {
                            if let Err(e) = self.register(manifest, Some(path.clone())).await {
                                warn!(path = %path.display(), error = %e, "Failed to register skill");
                            } else {
                                count += 1;
                            }
                        }
                        Err(e) => {
                            warn!(path = %manifest_path.display(), error = %e, "Failed to load manifest");
                        }
                    }
                }
            }
        }
        
        info!(count, dir = %dir.display(), "Loaded skills from directory");
        Ok(count)
    }
    
    /// Load a single manifest file
    async fn load_manifest(&self, path: &Path) -> Result<SkillManifest, SkillError> {
        let content = tokio::fs::read_to_string(path).await
            .map_err(|e| SkillError::Io(e.to_string()))?;
        
        let manifest: SkillManifest = serde_yaml::from_str(&content)
            .map_err(|e| SkillError::InvalidManifest(e.to_string()))?;
        
        Ok(manifest)
    }
    
    /// Get a skill by name
    pub async fn get(&self, name: &str) -> Option<Skill> {
        self.skills.read().await.get(name).cloned()
    }
    
    /// Find skill containing a specific tool
    pub async fn find_tool(&self, tool_name: &str) -> Option<(String, ToolDef)> {
        let skills = self.skills.read().await;
        for (skill_name, skill) in skills.iter() {
            if let Some(tool) = skill.manifest.tools.iter().find(|t| t.name == tool_name) {
                return Some((skill_name.clone(), tool.clone()));
            }
        }
        None
    }
    
    /// List all registered skills
    pub async fn list_skills(&self) -> Vec<SkillSummary> {
        self.skills
            .read()
            .await
            .values()
            .map(|s| SkillSummary {
                name: s.manifest.name.clone(),
                version: s.manifest.version.clone(),
                description: s.manifest.description.clone(),
                tool_count: s.manifest.tools.len(),
            })
            .collect()
    }
    
    /// List all available tools across all skills
    pub async fn list_tools(&self) -> Vec<ToolSummary> {
        let mut tools = Vec::new();
        let skills = self.skills.read().await;
        
        for skill in skills.values() {
            for tool in &skill.manifest.tools {
                tools.push(ToolSummary {
                    name: tool.name.clone(),
                    skill: skill.manifest.name.clone(),
                    description: tool.description.clone(),
                });
            }
        }
        
        tools
    }
    
    /// Get tool schema
    pub async fn get_tool_schema(&self, tool_name: &str) -> Option<serde_json::Value> {
        let (_, tool) = self.find_tool(tool_name).await?;
        Some(tool.parameters.clone())
    }
}

impl Default for SkillRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillSummary {
    pub name: String,
    pub version: String,
    pub description: String,
    pub tool_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolSummary {
    pub name: String,
    pub skill: String,
    pub description: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("IO error: {0}")]
    Io(String),
    #[error("Invalid manifest: {0}")]
    InvalidManifest(String),
    #[error("Duplicate tool name: {0}")]
    DuplicateTool(String),
    #[error("Skill not found: {0}")]
    NotFound(String),
    #[error("Tool not found: {0}")]
    ToolNotFound(String),
}
