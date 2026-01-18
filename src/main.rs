use clap::Parser;
use takeout_fixer::cli::Cli;
use takeout_fixer::run;

fn main() {
    let args = Cli::parse();
    run(args);
}
