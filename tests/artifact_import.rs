use hey_boss::artifacts::import::{collect, local_destinations, rewrite};
use std::collections::BTreeMap;

#[test]
fn rewrites_only_parsed_destinations_preserving_markdown() {
    let source = "![*Chart*](<chart one.png> \"Title\")\n[CSV][data]\n\n[data]: data.csv 'Download'\n\n`![code](chart one.png)`\n```md\n[x](data.csv)\n```\n[web](https://example.com)\n";
    let replacements = BTreeMap::from([
        ("chart one.png".into(), "/attachments/f-one".into()),
        ("data.csv".into(), "/attachments/f-two".into()),
    ]);
    let rewritten = rewrite(source, &replacements);
    assert!(rewritten.contains("![*Chart*](</attachments/f-one> \"Title\")"));
    assert!(rewritten.contains("[data]: /attachments/f-two 'Download'"));
    assert!(rewritten.contains("`![code](chart one.png)`"));
    assert!(rewritten.contains("[x](data.csv)"));
    assert!(rewritten.contains("https://example.com"));
}

#[test]
fn imports_relative_absolute_encoded_and_reference_paths_and_rejects_missing_files() {
    let dir = std::env::temp_dir().join(format!("hb-import-files-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a b.png"), [1, 2, 3]).unwrap();
    let body = format!(
        "![one](a%20b.png) ![again](a%20b.png)\n[absolute](<{}>)\n[ref][r]\n\n[r]: a%20b.png\n",
        dir.join("a b.png").display()
    );
    let files = collect(&body, &dir).unwrap();
    assert_eq!(files.len(), 2);
    assert!(
        files
            .iter()
            .all(|file| file.name == "a b.png" && file.data == "AQID")
    );
    assert!(
        collect("[missing](missing.pdf)", &dir)
            .unwrap_err()
            .to_string()
            .contains("missing.pdf")
    );
    assert!(collect("[directory](.)", &dir).is_err());
    assert!(local_destinations("[web](https://example.com/a) [mail](mailto:a@b.com) [section](#section) [hosted](/attachments/f-abc) [web](//example.com/a)").unwrap().is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn escaped_nested_and_unicode_destinations_follow_the_markdown_parser() {
    let body = "[Nested](assets/chart(one).svg) ![Escaped](assets/chart\\(two\\).svg) [名前](<日本語.png>)\n\n[x][Ref]\n\n[ref]: <a b.csv>\n";
    let found = local_destinations(body).unwrap();
    assert_eq!(
        found,
        vec![
            "a b.csv",
            "assets/chart(one).svg",
            "assets/chart(two).svg",
            "日本語.png"
        ]
    );
    let replacements = found
        .into_iter()
        .map(|url| (url, "/attachments/f-hosted".into()))
        .collect();
    assert_eq!(
        rewrite(body, &replacements)
            .matches("/attachments/f-hosted")
            .count(),
        4
    );
}
