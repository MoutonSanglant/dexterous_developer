use bevy_dexterous_developer::HotReloadOptions;

fn main() {
    println!("Main Thread: {:?}", std::thread::current().id());
    lib_bevy_dexterous_developer_example::bevy_main(HotReloadOptions::default());
}
