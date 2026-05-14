use std::env;

#[cfg(windows)]
fn main() {
    let mut resource = winresource::WindowsResource::new();

    let binary_name = env::var("CARGO_BIN_NAME").unwrap_or_else(|_| "ime-shift-fix".into());
    let product_name = title_case_identifier(&binary_name)
        .unwrap_or_else(|| env::var("CARGO_PKG_NAME").expect("CARGO_PKG_NAME is not set"));

    resource.set(
        "FileDescription",
        &env::var("CARGO_PKG_DESCRIPTION").expect("CARGO_PKG_DESCRIPTION is not set"),
    );
    resource.set("InternalName", &binary_name);
    resource.set(
        "LegalCopyright",
        &env::var("CARGO_PKG_LICENSE").expect("CARGO_PKG_LICENSE is not set"),
    );
    resource.set("OriginalFilename", &format!("{binary_name}.exe"));
    resource.set("ProductName", &product_name);

    resource
        .compile()
        .expect("failed to compile Windows resources");
}

#[cfg(windows)]
fn title_case_identifier(value: &str) -> Option<String> {
    let words = value
        .split(['-', '_'])
        .filter(|word| !word.is_empty())
        .map(|word| {
            if word.eq_ignore_ascii_case("ime") {
                return Some("IME".to_string());
            }

            let mut chars = word.chars();
            let first = chars.next()?.to_uppercase().collect::<String>();
            Some(format!("{first}{}", chars.as_str()))
        })
        .collect::<Option<Vec<_>>>()?;

    if words.is_empty() {
        None
    } else {
        Some(words.join(" "))
    }
}
