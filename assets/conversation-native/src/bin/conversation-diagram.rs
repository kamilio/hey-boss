use safe_bash_rust::render_dot_svg;
use std::io::{self, Read, Write};

fn graphviz(input: &str) -> Result<Vec<u8>, String> {
    render_dot_svg(
        input,
        &[("fontname", "Helvetica"), ("fontsize", "20")],
        &[
            ("fontname", "Helvetica"),
            ("fontsize", "20"),
            ("shape", "box"),
            ("style", "rounded,filled"),
            ("fillcolor", "#edf5ff"),
            ("color", "#7495b8"),
            ("fontcolor", "#203c58"),
        ],
        &[
            ("fontname", "Helvetica"),
            ("fontsize", "20"),
            ("color", "#637f96"),
        ],
    )
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let svg = match std::env::args().nth(1).as_deref() {
        Some("graphviz") => graphviz(&input)?,
        Some("mermaid") => mermaid_rs_renderer::render_with_options(
            &input,
            mermaid_rs_renderer::RenderOptions::modern(),
        )?
        .into_bytes(),
        _ => return Err("Expected graphviz or mermaid".into()),
    };
    io::stdout().write_all(&svg)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_node_style_edge_labels_and_font() {
        let svg = String::from_utf8(
            graphviz(r#"digraph { Source [fillcolor=gold]; Source -> Preview [label="résumé"]; }"#)
                .unwrap(),
        )
        .unwrap();
        for expected in [
            "<svg",
            ">Source<",
            ">Preview<",
            ">résumé<",
            "font-size=\"20",
            "fill=\"gold\"",
        ] {
            assert!(svg.contains(expected), "{svg}");
        }
    }
}
