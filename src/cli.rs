use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    /// Show accumulated overtime
    #[clap(long, short, action)]
    pub overtime: bool,
    #[clap(long, short, action)]
    pub daysums: Option<u64>,
}