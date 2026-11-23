use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    noctalia_rs_build::generate()
}
