use std::process::Command;

#[derive(serde::Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct Comment {
    author: Option<Author>,
    body: String,
    created_at: String,
}

#[derive(serde::Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct Issue {
    number: u32,
    title: String,
    state: String,
    author: Option<Author>,
    labels: Vec<Label>,
    updated_at: String,
    body: String,
    #[serde(default)]
    comments: Vec<Comment>,
}

#[derive(serde::Deserialize, Clone, Debug)]
struct Author {
    login: String,
}

#[derive(serde::Deserialize, Clone, Debug)]
struct Label {
    name: String,
    color: Option<String>,
}

fn clean_github_markdown(s: &str) -> String {
    let mut cleaned = String::new();
    let mut in_comment = false;
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if !in_comment && i + 4 <= chars.len() && chars[i..i+4] == ['<', '!', '-', '-'] {
            in_comment = true;
            i += 4;
            continue;
        }
        if in_comment && i + 3 <= chars.len() && chars[i..i+3] == ['-', '-', '>'] {
            in_comment = false;
            i += 3;
            continue;
        }
        if !in_comment {
            cleaned.push(chars[i]);
        }
        i += 1;
    }
    cleaned
        .replace("<sub>", "_")
        .replace("</sub>", "_")
        .replace("<sup>", "^")
        .replace("</sup>", "^")
        .replace("<strong>", "**")
        .replace("</strong>", "**")
        .replace("<i>", "_")
        .replace("</i>", "_")
        .replace("<em>", "_")
        .replace("</em>", "_")
        .replace("<code>", "`")
        .replace("</code>", "`")
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n")
}

fn main() {
    let output = Command::new("gh")
        .args(&[
            "issue",
            "view",
            "1009",
            "--repo",
            "aimRPG/aRPG-client",
            "--json",
            "number,title,state,author,labels,updatedAt,body,comments",
        ])
        .output()
        .expect("failed to run gh command");

    if !output.status.success() {
        println!("Error: {}", String::from_utf8_lossy(&output.stderr));
        return;
    }

    let issue: Issue = serde_json::from_slice(&output.stdout).unwrap();
    println!("SUCCESSFULLY parsed Issue #{}!", issue.number);

    println!("Rendering description markdown...");
    let cleaned_body = clean_github_markdown(&issue.body);
    let parsed_body = tui_markdown::from_str(&cleaned_body);
    println!("Description rendered successfully! Number of lines: {}", parsed_body.lines.len());

    println!("Rendering comments markdown...");
    for (idx, comment) in issue.comments.iter().enumerate() {
        println!("  Rendering comment #{}...", idx);
        let cleaned_comment = clean_github_markdown(&comment.body);
        let parsed_comment = tui_markdown::from_str(&cleaned_comment);
        println!("  Comment #{} rendered successfully! Number of lines: {}", idx, parsed_comment.lines.len());
    }

    println!("ALL RENDERED SUCCESSFULLY!");
}
