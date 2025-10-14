use std::io;

pub fn execute(_args: &[String]) -> io::Result<()> {
    println!("Available commands:");
    println!("  clear           - Clear the screen");
    println!("  ls [OPTIONS]    - List directory contents");
    println!("  pwd             - Print working directory");
    println!("  help            - Show this help message");
    println!("  exit            - Exit the shell");
    println!();
    println!("Use '<command> --help' for more information on a specific command");
    Ok(())
}
