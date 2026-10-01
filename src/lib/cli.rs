//! Command line options. Every option has the long name the README documents
//! plus the shorter spellings that were accepted before.

use clap::Parser;

#[derive(Debug, Parser, PartialEq, Eq)]
#[command(
    version,
    about = "Imports Halo actions from the CSV and Excel files in a directory"
)]
pub struct Cli {
    /// Parse and validate the files without posting anything
    #[arg(long = "only-parse-inputs", visible_alias = "only-parse", alias = "op")]
    pub only_parse: bool,

    /// Use the cached action IDs and skip fetching the reports
    #[arg(long = "only-use-cache", visible_alias = "only-cache", alias = "oc")]
    pub cache_only: bool,

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
                cache_only: false,
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
                "--only-use-cache",
                "--input-path",
                "input/1",
                "--batch-size",
                "10",
            ],
            [
                "--only-parse",
                "--only-cache",
                "--input",
                "input/1",
                "--batch",
                "10",
            ],
            ["--op", "--oc", "--ip", "input/1", "--bs", "10"],
        ] {
            let cli = parse(&args).unwrap();
            assert!(cli.only_parse && cli.cache_only, "{args:?}");
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
}
