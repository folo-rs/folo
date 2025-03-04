use many_cpus::ProcessorSet;

fn main() {
    for processor in ProcessorSet::all().processors() {
        println!("{processor:?}");
    }
}
