# Maestro Feature Specification: Dynamic Agent Orchestration & Volume Mobility

## 1. Project Overview
**Project Name:** Maestro (formerly ClawPen) 
**Objective:** Transform the orchestrator into an intelligent routing layer. When a user sends a message, the orchestrator must:
1. Classify the intent (using the Team/Router logic).
2. Determine if the task requires specific data volumes.
3. Ensure those volumes are mounted to the target specialist agent.
4. Route the task and return the response.

**Runtime Environment:** `exo` (Custom container runtime).

---

## 2. Phase 1: Exo Runtime Extension (Repository: `F:\Software\exo`)
The current `exo` runtime supports volume mounting only at container creation. To achieve "Maestro" mobility, `exo` requires a way to inject mounts into running containers.

### Task 1.1: Add `volume mount` Command
Add a new subcommand to the `exo` CLI to allow dynamic mounting.

**Proposed Command Syntax:**
```bash
exo volume mount <container_id_or_name> <host_path> <container_path> [--readonly]
```

**Requirements:**
- **CLI Implementation:** Update `crates/exo/src/main.rs` to include the `Volume` subcommand and `Mount` action.
- **Runtime Logic:** Implement the logic in the runtime backend to perform the mount. 
 - *Note:* Since `exo` targets WSL/Linux environments, this will likely involve utilizing `nsenter` or manipulating the container's namespace via the host to perform a bind-mount.
- **Verification:** Ensure the mount is visible inside the container (e.g., via `exo exec <id> ls <path>`).

---

## 3. Phase 2: Orchestrator Schema Updates (Repository: `F:\Software\Claw Pen\orchestrator`)
The orchestrator needs to track which volumes belong to which conversation sessions.

### Task 2.1: Update `src/types.rs`
Implement the following new structures:

```rust
/// Links a specific volume to a conversation session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionVolumeBinding {
 pub session_id: String,
 pub volume_id: String,
 pub target_path: String,
 pub agent_id: Option<String>, // The agent currently holding the volume
 pub expires_at: DateTime<Utc>,
}

/// Extension to ClassificationResult to support data-aware routing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassificationResult {
 pub intent: String,
 pub confidence: f32,
 pub matched_keywords: Vec<String>,
 pub needs_clarification: bool,
 pub required_volumes: Vec<String>, // List of Volume IDs required for this intent
}
```

---

## 4. Phase 3: Orchestration Middleware (Repository: `F:\Software\Claw Pen\orchestrator`)
This is the "Brain" of Maestro.

### Task 3.1: Create `src/orchestration.rs`
Implement an orchestration layer that sits between the API/WebSocket handlers and the Team Router.

**Workflow Logic:**
1. **Intercept:** Capture the incoming `chat.send` request.
2. **Classify:** Call `Team::router.classify(message)`.
3. **Evaluate Data Needs:**
 - Check `classification.required_volumes`.
 - Query `SessionVolumeManager` for existing bindings for this `session_id`.
4. **Execute Mobility:**
 - If `target_agent` does **not** have the required volume:
 - Call `exo volume mount <target_agent_id> <host_path> <target_path>`.
 - Update `SessionVolumeBinding` to reflect the new `agent_id`.
5. **Route:** Forward the message to the `target_agent` via `agent_comms`.
6. **Cleanup (Optional):** Implement a background task to unmount volumes from idle agents to prevent resource exhaustion.

---

## 5. Phase 4: Router Intelligence (Repository: `F:\Software\Claw Pen\orchestrator`)
The router must now be aware of the data requirements of various specialist roles.

### Task 4.1: Update `src/teams.rs`
Modify the `Router` to return volume requirements.

**Implementation Detail:**
- Update `TeamConfig` (TOML) to allow defining volume requirements per intent.
- Example TOML structure:
```toml
[routing.design_assistant]
keywords = ["revit", "model", "3d"]
required_volumes = ["project_cad_files"]
```
- Update `Router::classify` to include these `required_volumes` in the `ClassificationResult`.

---

## Summary of Handoff Deliverables

| Module | Change Type | Target File |
| :--- | :--- | :--- |
| **Exo CLI** | New Command | `crates/exo/src/main.rs` |
| **Exo Runtime** | New Feature | `crates/exo/src/commands/volume.rs` (suggested) |
| **Orchestrator Types**| Schema Update | `src/types.rs` |
| **Orchestrator Logic**| New Module | `src/orchestration.rs` |
| **Team Routing** | Logic Update | `src/teams.rs` |
| **Team Config** | Schema Update | `teams/*.toml` |
