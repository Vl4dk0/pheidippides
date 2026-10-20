use phi_core::{Client, Error, Result, SendOptions};
use phi_proto::Message;
use serde_json::{Value, json};

pub fn print(command: &str, mut data: Value) -> Result<()> {
    data["schema_version"] = json!(1);
    data["command"] = json!(command);
    println!("{}", serde_json::to_string(&data)?);
    Ok(())
}
pub fn error(message: &str, exit_code: u8) {
    println!(
        "{}",
        json!({"schema_version":1,"command":"error","error":message,"exit_code":exit_code,"trusted":false})
    );
}
pub fn receipt(client: &Client, message: &Message) -> Result<()> {
    print(
        if message.envelope.thread_id.is_some() {
            "reply"
        } else {
            "send"
        },
        json!({
            "thread_id":message.envelope.thread_id.unwrap_or(message.id),
            "message_id":message.id,
            "reference":client.cached_reference(message.id).ok().flatten().map(|r|r.to_string()),
        }),
    )
}
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
pub fn draft_error(
    client: &Client,
    error: Error,
    options: &SendOptions,
    command: &[&str],
) -> Result<Error> {
    if !matches!(error, Error::SummaryRequired(_)) {
        return Ok(error);
    }
    let draft = client
        .save_draft(&options.body)
        .map_err(|save| Error::User(format!("{error}\nfailed to save draft: {save}")))?;
    let mut retry = format!(
        "phi --space {} --as {} ",
        quote(&client.profile.space_id.to_string()),
        quote(client.handle.as_str())
    );
    retry.push_str(
        &command
            .iter()
            .map(|part| quote(part))
            .collect::<Vec<_>>()
            .join(" "),
    );
    retry.push_str(" - --summary \"…\"");
    for file in &options.files {
        retry.push_str(&format!(" --file {}", quote(&file.to_string_lossy())));
    }
    retry.push_str(&format!(" < {}", quote(&draft.to_string_lossy())));
    Ok(Error::User(format!(
        "{error}\nbody saved to draft: {}\nretry: {retry}",
        draft.display()
    )))
}
