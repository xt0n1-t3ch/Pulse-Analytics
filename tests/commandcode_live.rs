use cc_discord_presence::commandcode::{CLIENT_ID, Collector, Config, NAME};
use std::collections::HashSet;

#[test]
#[ignore = "Requires real Command Code CLI and Desktop history on the host"]
fn real_cli_and_desktop_history_share_one_adapter_without_duplicate_sessions() {
    let mut collector = Collector::default();
    let batch = collector.poll(&Config::default());
    assert!(batch.diagnostics.is_empty());
    let unique: HashSet<_> = batch.changed.iter().map(|session| &session.id).collect();
    assert_eq!(unique.len(), batch.changed.len());
    let desktop = batch
        .changed
        .iter()
        .filter(|session| session.desktop)
        .count();
    let cli = batch.changed.len() - desktop;
    assert!(desktop > 0, "No real Desktop sessions were found");
    assert!(cli > 0, "No real CLI sessions were found");
    assert_eq!(NAME, "Command Code");
    assert_eq!(CLIENT_ID, "1551026507806281829");
    collector.acknowledge(&batch);
    assert!(collector.poll(&Config::default()).changed.is_empty());
    println!(
        "Command Code real-history proof: {desktop} Desktop, {cli} CLI, no duplicate session IDs"
    );
}

#[test]
#[ignore = "Publishes an explicitly labelled diagnostic activity to local Discord"]
fn local_discord_acknowledges_command_code_name_and_published_asset() {
    use cc_discord_presence::commandcode::ASSET_KEY;
    use discord_rich_presence::{
        DiscordIpc, DiscordIpcClient,
        activity::{Activity, Assets},
    };
    let output =
        std::env::var("PULSE_COMMANDCODE_DISCORD_PROOF").expect("Explicit proof output required");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| -> anyhow::Result<serde_json::Value> {
            let mut client = DiscordIpcClient::new(CLIENT_ID);
            client.connect()?;
            client.set_activity(
                Activity::new()
                    .details("Pulse integration check")
                    .state("Command Code artwork verification")
                    .assets(Assets::new().large_image(ASSET_KEY).large_text(NAME)),
            )?;
            let (_, accepted) = client.recv()?;
            client.clear_activity()?;
            let (_, cleared) = client.recv()?;
            client.close()?;
            anyhow::ensure!(
                accepted["cmd"] == "SET_ACTIVITY" && accepted["evt"] != "ERROR",
                "Discord rejected the diagnostic activity"
            );
            anyhow::ensure!(
                accepted["data"]["name"] == NAME,
                "Discord returned a different application name"
            );
            anyhow::ensure!(
                accepted["data"]["application_id"] == CLIENT_ID,
                "Discord returned a different application ID"
            );
            anyhow::ensure!(
                cleared["cmd"] == "SET_ACTIVITY" && cleared["evt"] != "ERROR",
                "Discord did not acknowledge activity cleanup"
            );
            Ok(
                serde_json::json!({"application_id":CLIENT_ID,"name":accepted["data"]["name"],"assets":accepted["data"]["assets"],"diagnostic_activity":true,"set_activity_acknowledged":true,"clear_activity_acknowledged":true,"visual_rendering_verified":false}),
            )
        })();
        let _ = tx.send(result);
    });
    let result = rx
        .recv_timeout(std::time::Duration::from_secs(15))
        .expect("Discord IPC proof timed out")
        .expect("Discord IPC proof failed");
    std::fs::write(output, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
    println!("{result}");
}
