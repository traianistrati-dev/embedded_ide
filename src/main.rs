use eframe::egui;
pub mod app;
use app::AppIde;

pub mod build;
pub mod dfu;
pub mod espflash;
pub mod lsp;
pub mod openocd;
pub mod panels;
pub mod required_tools;
pub mod editor;
pub mod project_tree;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_maximized(true),
        ..Default::default()
    };

    eframe::run_native(
        "Embedded IDE",
        options,
        Box::new(|cc| Ok(Box::new(AppIde::new(cc)))),
    )
}
/////////
//NOT related to this project; Project TEST mutable references, ownership, borrowing, lifetimes, panics and smart pointers
#[test]
fn test_rust_code_smart_pointers() {}

#[test]
fn test_rust_code_ownership() {}

#[test]
fn test_rust_code() {
    let imutable_string: String = "123ABC".to_string();
    // let mutable_string: &mut String = &mut imutable_string.clone();
    let mutable_string: &mut String = &mut imutable_string.to_owned();

    println!("{:?} - &mut {:?}", imutable_string, mutable_string);
    println!("{:?} - &mut {:?}", imutable_string, mutable_string);
    println!("{:?} - &mut {:?}", imutable_string, mutable_string);

    use egui::TextBuffer;
    mutable_string.replace_with("456DEF");
    println!("{:?} - &mut {:?}", imutable_string, mutable_string);
    mutable_string.replace_with("456DEF-1");
    println!("{:?} - &mut {:?}", imutable_string, mutable_string);
    mutable_string.replace_with("456DEF-2");
    println!("{:?} - &mut {:?}", imutable_string, mutable_string);

    println!("{:?} - &mut {:?}", imutable_string, mutable_string);

    assert_ne!(
        // imutable_string, imutable_string,
        mutable_string.to_string(),
        "OK if the values are not equal"
    );

    if true {
        //TODO:
    } else {
        std::panic!(
            "imutable_string {:?} - {:?}\n mutable_string {:?} - {:?}",
            imutable_string,
            mutable_string,
            imutable_string,
            mutable_string
        );
    }
}
