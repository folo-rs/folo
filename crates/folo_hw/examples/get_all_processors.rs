use folo_hw::ProcessorSet;

fn main() {
    for processor in ProcessorSet::all().processors() {
        println!("{:?}", processor);
    }
}