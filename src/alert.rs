use anyhow::Result;

pub async fn send(client: &reqwest::Client, txtme_url: &str, message: &str) -> Result<()> {
    client
        .post(txtme_url)
        .json(&serde_json::json!({ "message": message }))
        .send()
        .await?;
    Ok(())
}
