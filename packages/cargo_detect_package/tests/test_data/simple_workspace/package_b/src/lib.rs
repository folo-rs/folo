use package_a::hello_from_a;

pub fn hello_from_b() {
    println!("Hello from package B!");
    hello_from_a();
}
