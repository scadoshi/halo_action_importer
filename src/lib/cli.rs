//! Command line options. Each has the long name the README documents, and
//! most have two shorter spellings.

use clap::Parser;

/// The parsed command line.
#[derive(Debug, Parser, PartialEq, Eq)]
#[command(
    version,
    about = "Imports Halo actions from the CSV and Excel files in a directory"
)]
pub struct Cli {
    /// Parse and validate the files without posting anything
    #[arg(long = "only-parse-inputs", visible_alias = "only-parse", alias = "op")]
    pub only_parse: bool,

    /// Fetch the imported action IDs from the Halo reports and merge them into
    /// the cache before importing. Without it, the cache alone says what Halo has
    #[arg(long = "sync-from-halo", visible_alias = "sync", alias = "sfh")]
    pub sync_from_halo: bool,

    /// Start a new import with an empty cache, for an instance that has no
    /// imported actions yet. A run with an empty cache refuses without it
    #[arg(long = "fresh", conflicts_with = "sync_from_halo")]
    pub fresh: bool,

    /// Directory holding the files to import
    #[arg(
        long = "input-path",
        visible_alias = "input",
        alias = "ip",
        default_value = "input"
    )]
    pub input_path: String,

    /// Actions per request, at least 1
    #[arg(
        long = "batch-size",
        visible_alias = "batch",
        alias = "bs",
        default_value_t = 1,
        value_parser = clap::value_parser!(u16).range(1..)
    )]
    pub batch_size: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("halo_action_importer").chain(args.iter().copied()))
    }

    #[test]
    fn no_arguments_means_a_real_import_of_the_input_directory_one_at_a_time() {
        let cli = parse(&[]).unwrap();
        assert_eq!(
            cli,
            Cli {
                only_parse: false,
                sync_from_halo: false,
                fresh: false,
                input_path: "input".to_string(),
                batch_size: 1,
            }
        );
    }

    #[test]
    fn every_spelling_of_an_option_parses_the_same() {
        for args in [
            [
                "--only-parse-inputs",
                "--sync-from-halo",
                "--input-path",
                "input/1",
                "--batch-size",
                "10",
            ],
            [
                "--only-parse",
                "--sync",
                "--input",
                "input/1",
                "--batch",
                "10",
            ],
            ["--op", "--sfh", "--ip", "input/1", "--bs", "10"],
        ] {
            let cli = parse(&args).unwrap();
            assert!(cli.only_parse && cli.sync_from_halo, "{args:?}");
            assert_eq!(cli.input_path, "input/1", "{args:?}");
            assert_eq!(cli.batch_size, 10, "{args:?}");
        }
    }

    #[test]
    fn a_batch_size_that_is_not_a_positive_number_is_rejected() {
        for bad in ["abc", "0", "-1", ""] {
            assert!(parse(&["--batch-size", bad]).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn an_option_without_its_value_is_rejected() {
        assert!(parse(&["--input-path"]).is_err());
        assert!(parse(&["--batch-size"]).is_err());
    }

    #[test]
    fn an_unknown_flag_is_rejected_instead_of_ignored() {
        assert!(parse(&["--only-parse-input"]).is_err());
        assert!(parse(&["extra"]).is_err());
    }

    #[test]
    fn fresh_parses_and_cannot_be_combined_with_a_sync() {
        assert!(parse(&["--fresh"]).unwrap().fresh);
        assert!(parse(&["--fresh", "--sync-from-halo"]).is_err());
    }

    #[test]
    fn the_cache_only_flag_is_gone_since_the_cache_is_the_default() {
        for flag in ["--only-use-cache", "--only-cache", "--oc"] {
            assert!(parse(&[flag]).is_err(), "{flag}");
        }
    }
}
