use crate::{
    session::{
        CancelRunResult, PreflightSnapshot, RunResponse, RunTicket, Session, SessionSnapshot,
    },
    workspace::SaveResult,
};
use serde::Serialize;
use std::sync::Arc;
use tauri::State;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveSourceResponse {
    pub revision: u64,
    pub source_digest: String,
    pub snapshot: SessionSnapshot,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HintResponse {
    pub exercise_id: String,
    pub hint: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SolutionResponse {
    pub exercise_id: String,
    pub solution: String,
}

#[tauri::command]
pub fn session_snapshot(session: State<'_, Arc<Session>>) -> Result<SessionSnapshot, String> {
    session.snapshot().map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn retry_preflight(
    session: State<'_, Arc<Session>>,
) -> Result<PreflightSnapshot, String> {
    Ok(session.retry_preflight().await)
}

#[tauri::command(rename_all = "camelCase")]
pub fn save_source(
    session: State<'_, Arc<Session>>,
    exercise_id: String,
    expected_revision: u64,
    source: String,
) -> Result<SaveSourceResponse, String> {
    let SaveResult { revision, digest } = session
        .save_source(&exercise_id, expected_revision, &source)
        .map_err(|error| error.to_string())?;
    Ok(SaveSourceResponse {
        revision,
        source_digest: digest,
        snapshot: session.snapshot().map_err(|error| error.to_string())?,
    })
}

#[tauri::command(rename_all = "camelCase")]
pub fn select_exercise(
    session: State<'_, Arc<Session>>,
    exercise_id: String,
) -> Result<SessionSnapshot, String> {
    session
        .select_exercise(&exercise_id)
        .map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn reveal_hint(
    session: State<'_, Arc<Session>>,
    exercise_id: String,
) -> Result<HintResponse, String> {
    let hint = session
        .reveal_hint(&exercise_id)
        .map_err(|error| error.to_string())?;
    Ok(HintResponse { exercise_id, hint })
}

#[tauri::command(rename_all = "camelCase")]
pub fn reveal_solution(
    session: State<'_, Arc<Session>>,
    exercise_id: String,
) -> Result<SolutionResponse, String> {
    let solution = session
        .reveal_solution(&exercise_id)
        .map_err(|error| error.to_string())?;
    Ok(SolutionResponse {
        exercise_id,
        solution,
    })
}

#[tauri::command(rename_all = "camelCase")]
pub fn run_exercise(
    session: State<'_, Arc<Session>>,
    exercise_id: String,
) -> Result<RunTicket, String> {
    session
        .inner()
        .start_run(&exercise_id)
        .map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn run_result(
    session: State<'_, Arc<Session>>,
    run_id: String,
) -> Result<RunResponse, String> {
    session
        .await_run(&run_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn cancel_run(
    session: State<'_, Arc<Session>>,
    run_id: String,
) -> Result<CancelRunResult, String> {
    Ok(session.cancel_run(&run_id).await)
}
