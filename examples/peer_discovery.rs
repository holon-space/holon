use anyhow::Result;
use holon::sync::LoroDocument;
#[cfg(feature = "iroh-sync")]
use holon::sync::IrohSyncAdapter;
use std::env;
use tracing_subscriber;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        println!("Usage:");
        println!("  cargo run --example peer_discovery --features iroh-sync <doc_id> [peer_node_id]");
        println!();
        println!("Examples:");
        println!("  # First peer (will wait for others to connect):");
        println!("  cargo run --example peer_discovery --features iroh-sync project-notes");
        println!();
        println!("  # Second peer (connects to first peer):");
        println!("  cargo run --example peer_discovery --features iroh-sync project-notes <node_id_from_first_peer>");
        println!();
        println!("Both peers can send AND receive - this is true P2P!");
        println!("The <doc_id> must match on all peers to collaborate.");
        return Ok(());
    }

    let doc_id = &args[1];
    let peer_node_id = args.get(2);

    run_peer(doc_id, peer_node_id.map(|s| s.as_str())).await
}

#[cfg(feature = "iroh-sync")]
async fn run_peer(doc_id: &str, peer_node_id: Option<&str>) -> Result<()> {
    println!("=== Starting P2P Peer ===");
    println!("Document: '{}'", doc_id);
    println!();

    let mut doc = LoroDocument::new(doc_id.to_string())?;
    let adapter = IrohSyncAdapter::new("loro-sync").await?;
    adapter.set_peer_id_from_node(&mut doc)?;

    println!("Peer ready!");
    println!("My Node ID: {}", adapter.node_id());
    println!();

    let initial_text = format!("Hello from peer {}! ", &adapter.node_id().to_string()[..8]);
    doc.insert_text("editor", 0, &initial_text).await?;
    println!("Initial text: {}", doc.get_text("editor").await?);
    println!();

    match peer_node_id {
        Some(peer_id_str) => {
            println!("Connecting to peer: {}", peer_id_str);
            println!();

            let peer_public_key: iroh::PublicKey = peer_id_str.parse()?;
            let peer_addr = iroh::NodeAddr::new(peer_public_key);

            adapter.sync_with_peer(&doc, peer_addr).await?;

            println!("Sync complete!");
            println!("After sync: {}", doc.get_text("editor").await?);
        }
        None => {
            println!("Waiting for other peers to connect...");
            println!("   Share this command with other peers:");
            println!(
                "   cargo run --example peer_discovery --features iroh-sync {} {}",
                doc_id,
                adapter.node_id()
            );
            println!();

            adapter.accept_sync(&doc).await?;

            println!();
            println!("Received sync from peer!");
            println!("After sync: {}", doc.get_text("editor").await?);
        }
    }

    Ok(())
}

#[cfg(not(feature = "iroh-sync"))]
async fn run_peer(_doc_id: &str, _peer_node_id: Option<&str>) -> Result<()> {
    eprintln!("This example requires the 'iroh-sync' feature.");
    eprintln!("Run with: cargo run --example peer_discovery --features iroh-sync");
    std::process::exit(1);
}
