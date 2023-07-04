use serde::{Deserialize, Serialize};
use serde_yaml;
use std::fs;
use tonic_build::{compile_protos, configure};

use config::Config;

struct ExchangeMacro {
    macro_name: &'static str,
    file_location: &'static str,
    search_pattern: &'static str,
    replacement_value: &'static str,
}

const CONFIG_LOCATION: &'static str = "orderbook-quoter-server/integrative-testing-config.yaml";

const EXCHANGE_MACROS: [ExchangeMacro; 1] = [ExchangeMacro {
    macro_name: "orderbook_levels",
    file_location: "orderbook/src/lib.rs",
    search_pattern: "new_level!(6)",
    replacement_value: "new_level!(N)",
}];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let current_dir = std::env::current_dir().expect("Failed to get current directory");
    let config_path = current_dir.join(&CONFIG_LOCATION);
    let config_content = fs::read_to_string(config_path)?;
    let config: Config = serde_yaml::from_str(&config_content)?;
    assert!(config.exchanges.len() == config.orderbook.exchange_count as usize);
    let exchange_count = config.orderbook.exchange_count;

    for mut replacements in EXCHANGE_MACROS {
        if replacements.macro_name == "orderbook_levels" {
            replacements
                .replacement_value
                .replace("N", exchange_count.to_string().clone().as_str());
        }

        let file_path = current_dir.join(replacements.file_location);

        // Read the file contents
        let mut file_contents = fs::read_to_string(&file_path).expect("Failed to read file");

        // Perform the search and replace
        file_contents =
            file_contents.replace(replacements.search_pattern, replacements.replacement_value);

        // Write the modified contents back to the file
        fs::write(&file_path, file_contents).expect("Failed to write file");

        println!(
            "Search and replace completed in file: {}",
            file_path.display()
        );
    }

    tonic_build::configure()
        .build_server(true)
        .out_dir("proto-source")
        .compile(&["proto/quote/streaming.proto"], &["proto/"])?;
    Ok(())
}
