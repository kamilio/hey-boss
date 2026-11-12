use super::*;

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn slash_edits_skill_markdown_and_only_opens_search_outside_the_editor() {
    let data = inventory(
        &[skill("example", "original", "# Example\nOriginal content")],
        "example",
    );
    with_page(data, |p, f, url| {
        p.command("Page.navigate", json!({"url":url})).unwrap();
        click(p, "[data-open-native-editor]");
        p.wait("document.querySelector('[data-open-native-editor]')?.disabled===false");
        assert_eq!(
            f.lock().unwrap().applied.last().unwrap()["action"],
            "open_editor"
        );
        edit(p, "A path");
        key(p, "/", "Slash", 191, "/");
        assert_eq!(p.eval(&format!("{EDITOR}.innerText")).unwrap(), "A path/");
        assert_eq!(
            p.eval("document.activeElement.id === 'skills-search'")
                .unwrap(),
            false
        );
        for (name, width, theme) in [
            ("desktop", 1440, "light"),
            ("dark", 1440, "dark"),
            ("mobile", 390, "light"),
        ] {
            viewport(p, width, theme);
            p.eval("document.querySelector('.skill-markdown-editor-card').scrollIntoView({block:'center'})").unwrap();
            shot(p, &format!("skill-edit-{name}"), None);
        }
        click(p, "[data-save-markdown=distribute]");
        p.wait("!document.querySelector('[data-save-markdown]') && !document.querySelector('[data-open-native-editor]').disabled");
        let saved = f.lock().unwrap().applied.last().unwrap().clone();
        assert_eq!(saved["content"], "A path/");
        assert_eq!(saved["distribute"], true);
        p.eval("document.querySelector('#skills-main').focus()")
            .unwrap();
        key(p, "/", "Slash", 191, "/");
        assert_eq!(
            p.eval("document.activeElement.id").unwrap(),
            "skills-search"
        );
    });
}
