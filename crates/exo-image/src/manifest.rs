//! Agent manifest (`exo.toml`) — Exo's agent-native alternative to a Dockerfile
//! (E2).
//!
//! A Dockerfile only describes *how to build a filesystem*. An agent needs more
//! than that: which tools it may call, what it's allowed to reach on the network,
//! and its resource budget — all of which Exo enforces at runtime. The manifest
//! captures the build *and* that runtime policy in one declarative file:
//!
//! ```toml
//! [agent]
//! name = "researcher"
//! from = "python:3.12-slim"     # base image
//!
//! [build]
//! workdir = "/app"
//! copy = [["./src", "/app"]]     # host -> image
//! run  = ["pip install -r requirements.txt"]
//! env  = { LOG_LEVEL = "info" }
//! cmd  = ["python", "main.py"]
//!
//! [tools]
//! allow = ["bash", "web"]        # tool-bus capabilities the agent may use
//!
//! [resources]
//! memory = "512M"
//! cpu = "1"
//!
//! [egress]
//! allow = ["api.anthropic.com"]  # default-deny; only these hosts reachable
//! ```

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// A parsed `exo.toml` agent manifest.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentManifest {
    pub agent: AgentSection,
    #[serde(default)]
    pub build: BuildSection,
    #[serde(default)]
    pub tools: ToolsSection,
    #[serde(default)]
    pub resources: ResourcesSection,
    #[serde(default)]
    pub egress: EgressSection,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentSection {
    /// Image/agent name (used as the built image's repository).
    pub name: String,
    /// Base image to build from, e.g. `python:3.12-slim`.
    pub from: String,
    /// Optional tag for the built image (defaults to "latest").
    #[serde(default)]
    pub tag: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BuildSection {
    /// Working directory set in the image.
    #[serde(default)]
    pub workdir: Option<String>,
    /// `[host, dest]` pairs copied into the image.
    #[serde(default)]
    pub copy: Vec<[String; 2]>,
    /// Shell commands run, in order, during build.
    #[serde(default)]
    pub run: Vec<String>,
    /// Environment variables baked into the image.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Default command (argv) the agent runs.
    #[serde(default)]
    pub cmd: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolsSection {
    /// Tool-bus capabilities this agent is permitted to use.
    #[serde(default)]
    pub allow: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResourcesSection {
    /// Memory limit, e.g. "512M", "2G".
    #[serde(default)]
    pub memory: Option<String>,
    /// CPU limit, e.g. "1", "200%".
    #[serde(default)]
    pub cpu: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EgressSection {
    /// Allowed egress hosts. Empty list = default-deny (no network egress).
    #[serde(default)]
    pub allow: Vec<String>,
}

impl AgentManifest {
    /// Parse a manifest from TOML text, validating required fields.
    pub fn parse(text: &str) -> Result<Self> {
        let manifest: AgentManifest =
            toml::from_str(text).context("invalid exo.toml syntax")?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Load and parse a manifest file from disk.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading manifest {:?}", path))?;
        Self::parse(&text)
    }

    /// Parse a Dockerfile subset into an agent manifest, so existing Dockerfiles
    /// flow through the same build path. Supported: FROM, RUN, COPY/ADD, ENV,
    /// CMD, WORKDIR. `name` becomes the built image's name (e.g. from `-t`).
    /// Line continuations (`\`) and `#` comments are handled.
    pub fn from_dockerfile(text: &str, name: &str) -> Result<Self> {
        let mut m = AgentManifest::default();
        m.agent.name = name.to_string();

        // Join continued lines, drop comments/blanks.
        let mut logical: Vec<String> = Vec::new();
        let mut acc = String::new();
        for raw in text.lines() {
            let line = raw.trim_end();
            let trimmed = line.trim_start();
            if !acc.is_empty() {
                acc.push(' ');
            }
            if let Some(stripped) = line.strip_suffix('\\') {
                acc.push_str(stripped.trim_start());
                continue;
            }
            if acc.is_empty() {
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                logical.push(trimmed.to_string());
            } else {
                acc.push_str(trimmed);
                logical.push(acc.trim().to_string());
                acc.clear();
            }
        }
        if !acc.is_empty() {
            logical.push(acc.trim().to_string());
        }

        for line in logical {
            let (instr, rest) = match line.split_once(char::is_whitespace) {
                Some((i, r)) => (i.to_uppercase(), r.trim().to_string()),
                None => (line.to_uppercase(), String::new()),
            };
            match instr.as_str() {
                "FROM"
                    // Ignore "AS <stage>" aliases; first FROM wins.
                    if m.agent.from.is_empty() => {
                        m.agent.from = rest.split_whitespace().next().unwrap_or("").to_string();
                    }
                "RUN" => m.build.run.push(rest),
                "WORKDIR" => m.build.workdir = Some(rest),
                "ENV" => {
                    for (k, v) in parse_env(&rest) {
                        m.build.env.insert(k, v);
                    }
                }
                "COPY" | "ADD" => {
                    let parts: Vec<&str> = rest.split_whitespace().collect();
                    if parts.len() >= 2 {
                        // Last token is dest; everything before is sources.
                        let dst = parts[parts.len() - 1].to_string();
                        for src in &parts[..parts.len() - 1] {
                            m.build.copy.push([src.to_string(), dst.clone()]);
                        }
                    }
                }
                "CMD" | "ENTRYPOINT" => m.build.cmd = parse_argv(&rest),
                _ => { /* skip unsupported instructions (LABEL, EXPOSE, ...) */ }
            }
        }
        m.validate()?;
        Ok(m)
    }

    /// The built image's tag (defaults to "latest").
    pub fn tag(&self) -> &str {
        self.agent.tag.as_deref().unwrap_or("latest")
    }

    /// Full reference for the built image, e.g. "researcher:latest".
    pub fn image_reference(&self) -> String {
        format!("{}:{}", self.agent.name, self.tag())
    }

    fn validate(&self) -> Result<()> {
        if self.agent.name.trim().is_empty() {
            anyhow::bail!("[agent].name is required");
        }
        if self.agent.from.trim().is_empty() {
            anyhow::bail!("[agent].from (base image) is required");
        }
        // A copy with an empty source or dest is almost always a typo.
        for [src, dst] in &self.build.copy {
            if src.trim().is_empty() || dst.trim().is_empty() {
                anyhow::bail!("[build].copy entries need non-empty [host, dest]");
            }
        }
        Ok(())
    }

    /// Render a human-readable build plan (what `exo build` will do).
    pub fn plan(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("Build {} from {}\n", self.image_reference(), self.agent.from));
        if let Some(wd) = &self.build.workdir {
            out.push_str(&format!("  workdir: {}\n", wd));
        }
        for [s, d] in &self.build.copy {
            out.push_str(&format!("  copy: {} -> {}\n", s, d));
        }
        for r in &self.build.run {
            out.push_str(&format!("  run: {}\n", r));
        }
        if !self.build.cmd.is_empty() {
            out.push_str(&format!("  cmd: {}\n", self.build.cmd.join(" ")));
        }
        if !self.tools.allow.is_empty() {
            out.push_str(&format!("  tools: {}\n", self.tools.allow.join(", ")));
        }
        match self.egress.allow.is_empty() {
            true => out.push_str("  egress: default-deny (no hosts allowed)\n"),
            false => out.push_str(&format!("  egress: {}\n", self.egress.allow.join(", "))),
        }
        out
    }
}

/// Parse a CMD/ENTRYPOINT value: exec form `["a","b"]` or shell form `a b`.
fn parse_argv(rest: &str) -> Vec<String> {
    let t = rest.trim();
    if t.starts_with('[') {
        if let Ok(v) = serde_json::from_str::<Vec<String>>(t) {
            return v;
        }
    }
    t.split_whitespace().map(str::to_string).collect()
}

/// Parse ENV in both `KEY=VALUE [KEY2=VALUE2 ...]` and legacy `KEY value` forms.
fn parse_env(rest: &str) -> Vec<(String, String)> {
    let t = rest.trim();
    if t.contains('=') {
        t.split_whitespace()
            .filter_map(|kv| kv.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
            .collect()
    } else if let Some((k, v)) = t.split_once(char::is_whitespace) {
        vec![(k.to_string(), v.trim().to_string())]
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
        [agent]
        name = "researcher"
        from = "python:3.12-slim"

        [build]
        workdir = "/app"
        copy = [["./src", "/app"]]
        run = ["pip install requests"]
        cmd = ["python", "main.py"]
        env = { LOG_LEVEL = "info" }

        [tools]
        allow = ["bash", "web"]

        [resources]
        memory = "512M"
        cpu = "1"

        [egress]
        allow = ["api.anthropic.com"]
    "#;

    #[test]
    fn parses_full_manifest() {
        let m = AgentManifest::parse(SAMPLE).unwrap();
        assert_eq!(m.agent.name, "researcher");
        assert_eq!(m.agent.from, "python:3.12-slim");
        assert_eq!(m.image_reference(), "researcher:latest");
        assert_eq!(m.build.copy[0], ["./src".to_string(), "/app".to_string()]);
        assert_eq!(m.build.env.get("LOG_LEVEL").unwrap(), "info");
        assert_eq!(m.tools.allow, vec!["bash", "web"]);
        assert_eq!(m.egress.allow, vec!["api.anthropic.com"]);
    }

    #[test]
    fn minimal_manifest_defaults() {
        let m = AgentManifest::parse(
            "[agent]\nname = \"a\"\nfrom = \"alpine:3.20\"\n",
        )
        .unwrap();
        assert_eq!(m.tag(), "latest");
        assert!(m.build.run.is_empty());
        // No egress section => default-deny.
        assert!(m.egress.allow.is_empty());
    }

    #[test]
    fn rejects_missing_required_fields() {
        assert!(AgentManifest::parse("[agent]\nname = \"\"\nfrom = \"x\"\n").is_err());
        assert!(AgentManifest::parse("[agent]\nname = \"a\"\nfrom = \"\"\n").is_err());
    }

    #[test]
    fn parses_dockerfile_subset() {
        let df = r#"
            # a comment
            FROM python:3.12-slim AS base
            WORKDIR /app
            COPY ./src /app
            ENV LOG_LEVEL=info APP=demo
            RUN pip install requests \
                && pip install flask
            CMD ["python", "main.py"]
            EXPOSE 8080
        "#;
        let m = AgentManifest::from_dockerfile(df, "demo").unwrap();
        assert_eq!(m.agent.name, "demo");
        assert_eq!(m.agent.from, "python:3.12-slim"); // AS base stripped
        assert_eq!(m.build.workdir.as_deref(), Some("/app"));
        assert_eq!(m.build.copy[0], ["./src".to_string(), "/app".to_string()]);
        assert_eq!(m.build.env.get("LOG_LEVEL").unwrap(), "info");
        assert_eq!(m.build.env.get("APP").unwrap(), "demo");
        // Line continuation joined into one RUN.
        assert_eq!(m.build.run.len(), 1);
        assert!(m.build.run[0].contains("flask"));
        assert_eq!(m.build.cmd, vec!["python", "main.py"]); // exec form
    }

    #[test]
    fn dockerfile_requires_from() {
        assert!(AgentManifest::from_dockerfile("RUN echo hi\n", "x").is_err());
    }

    #[test]
    fn plan_mentions_default_deny_egress() {
        let m = AgentManifest::parse("[agent]\nname=\"a\"\nfrom=\"alpine:3.20\"\n").unwrap();
        assert!(m.plan().contains("default-deny"));
    }
}
