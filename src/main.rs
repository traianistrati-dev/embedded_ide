use eframe::egui;
pub mod app;
use app::AppIde;
use egui::debug_text::print;

pub mod build;
pub mod dfu;
pub mod editor;
pub mod espflash;
pub mod lsp;
pub mod openocd;
pub mod panels;
pub mod project_tree;
pub mod required_tools;

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
fn test_rust_code_smart_pointers() {
    let ssss = "123ABC".to_owned();

    let mut smart_pointer: *const String = &ssss;

    smart_pointer = &"xxxxxx".to_string();

    update_smart_pointer(&mut smart_pointer);
    update_smart_pointer2(smart_pointer);
    if true {
        //TODO:
    } else {
        std::panic!(
            "smart_pointer {:?}, {:p}, {:?}",
            smart_pointer.clone(),
            smart_pointer.clone(),
            unsafe { &*smart_pointer }
        );
    }

    fn update_smart_pointer(smart_pointer: &mut *const String) {
        unsafe {
            println!(
                "update_smart_pointer {:?}, *{:?}",
                smart_pointer,
                // (&***smart_pointer as &str)
                // (&**smart_pointer as &String)
                (**smart_pointer)
            );
        }

        *smart_pointer = Box::leak(Box::new("kkkkk".to_string()));

        unsafe {
            println!(
                "update_smart_pointer {:?}, *{:?}",
                smart_pointer,
                (**smart_pointer)
            );
        }
    }
    /////
    fn update_smart_pointer2(text: *const String) {
        let smart_pointer = text.cast_mut();
        unsafe {
            println!(
                "update_smart_pointer2 {:?}, *{:?}",
                smart_pointer,
                (*smart_pointer)
            );

            {
                // (*smart_pointer).push_str("ooooo");
                (*smart_pointer) = "ooooo".to_owned();
            }

            println!(
                "update_smart_pointer2 {:?}, *{:?}",
                smart_pointer,
                (*smart_pointer)
            );
        };
    }
}

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
