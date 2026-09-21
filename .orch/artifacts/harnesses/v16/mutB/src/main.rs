//! CLI 入口：只做参数解析、`-C` 切换目录、错误打印。业务逻辑全在 `minigit::cli`。
//! CONTROLLER-OWNED 文件。

use clap::Parser;

use minigit::cli::{self, Cli};

fn main() {
    let cli = Cli::parse();
    if let Err(err) = cli::dispatch(cli) {
        eprintln!("mg: fatal: {err}");
        std::process::exit(1);
    }
}
