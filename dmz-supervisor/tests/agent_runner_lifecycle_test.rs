use dmz_supervisor::{AgentConfig, AgentRunner};
use std::collections::HashMap;
use tokio::time::{sleep, Duration};

#[tokio::test]
async fn test_agent_runner_retains_scratchpad_while_process_running() {
    let runner = AgentRunner::new();
    let agent_id = "test-agent-lifecycle".to_string();

    let cmd = if cfg!(windows) { "cmd.exe" } else { "sleep" };
    let args = if cfg!(windows) {
        vec!["/C".to_string(), "timeout /t 2 /nobreak".to_string()]
    } else {
        vec!["1".to_string()]
    };

    let config = AgentConfig {
        agent_id: agent_id.clone(),
        binary_or_script: cmd.to_string(),
        args,
        envs: HashMap::new(),
        allowed_domains: vec!["api.openai.com".to_string()],
        scratchpad_bytes: 1024 * 1024,
    };

    runner
        .spawn_agent(config)
        .await
        .expect("Failed to spawn agent");

    // Agent must be registered in active agents
    let active = runner.list_active_agents().await;
    assert_eq!(active, vec![agent_id.clone()]);

    let scratchpad_path = std::env::temp_dir().join(format!("dmz_agent_scratchpad_{}", agent_id));
    assert!(
        scratchpad_path.exists(),
        "Scratchpad must exist while agent is running"
    );

    // Explicitly terminate the agent
    runner
        .terminate_agent(&agent_id)
        .await
        .expect("Terminate must succeed");

    // Give asynchronous watcher time to process cleanup
    sleep(Duration::from_millis(300)).await;

    // Agent must no longer be in list_active_agents
    let active_after = runner.list_active_agents().await;
    assert!(
        active_after.is_empty(),
        "Active agents list should be empty after termination"
    );

    assert!(
        !scratchpad_path.exists(),
        "Scratchpad must be deleted on termination"
    );
}
