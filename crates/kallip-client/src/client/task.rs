//! Task-domain client methods for [`TagmaClient`].
//!
//! Mirrors the task routes served by `kallip-tagma` under `/tasks`.

use super::TagmaClient;
use anyhow::{Context, Result};
use kallip_common::protocol::{
    TaskChainOpRequest, TaskCheckpointRequest, TaskCloseRequest, TaskCreateRequest,
    TaskDispatchRequest, TaskExport, TaskForceRequest, TaskListQuery, TaskNoteRequest,
};

impl TagmaClient {
    pub async fn task_create(&self, req: &TaskCreateRequest) -> Result<TaskExport> {
        self.handle_response(
            self.with_auth(self.inner.http.post(self.url("/tasks")).json(&req))
                .send()
                .await
                .context("create task")?,
            "parse task export",
        )
        .await
    }

    pub async fn task_start(&self, id: i64, req: &TaskForceRequest) -> Result<TaskExport> {
        self.handle_response(
            self.with_auth(
                self.inner
                    .http
                    .post(self.url(&format!("/tasks/{id}/start")))
                    .json(&req),
            )
            .send()
            .await
            .context("start task")?,
            "parse task export",
        )
        .await
    }

    pub async fn task_list(&self, query: &TaskListQuery) -> Result<Vec<TaskExport>> {
        self.handle_response(
            self.with_auth(self.inner.http.get(self.url("/tasks")).query(query))
                .send()
                .await
                .context("list tasks")?,
            "parse task list",
        )
        .await
    }
    pub async fn task_show(&self, id: i64) -> Result<TaskExport> {
        self.handle_response(
            self.with_auth(self.inner.http.get(self.url(&format!("/tasks/{id}"))))
                .send()
                .await
                .context("show task")?,
            "parse task export",
        )
        .await
    }

    pub async fn task_export_all(&self) -> Result<Vec<TaskExport>> {
        self.handle_response(
            self.with_auth(self.inner.http.get(self.url("/tasks/export")))
                .send()
                .await
                .context("export tasks")?,
            "parse task list",
        )
        .await
    }

    pub async fn task_checkpoint(
        &self,
        id: i64,
        req: &TaskCheckpointRequest,
    ) -> Result<TaskExport> {
        self.handle_response(
            self.with_auth(
                self.inner
                    .http
                    .post(self.url(&format!("/tasks/{id}/checkpoint")))
                    .json(&req),
            )
            .send()
            .await
            .context("checkpoint task")?,
            "parse task export",
        )
        .await
    }

    pub async fn task_annotate(&self, id: i64, req: &TaskNoteRequest) -> Result<TaskExport> {
        self.handle_response(
            self.with_auth(
                self.inner
                    .http
                    .post(self.url(&format!("/tasks/{id}/annotate")))
                    .json(&req),
            )
            .send()
            .await
            .context("annotate task")?,
            "parse task export",
        )
        .await
    }

    pub async fn task_gate_report(&self, id: i64, req: &TaskNoteRequest) -> Result<TaskExport> {
        self.handle_response(
            self.with_auth(
                self.inner
                    .http
                    .post(self.url(&format!("/tasks/{id}/gate-report")))
                    .json(&req),
            )
            .send()
            .await
            .context("record gate report")?,
            "parse task export",
        )
        .await
    }

    pub async fn task_dispatch(&self, id: i64, req: &TaskDispatchRequest) -> Result<TaskExport> {
        self.handle_response(
            self.with_auth(
                self.inner
                    .http
                    .post(self.url(&format!("/tasks/{id}/dispatch")))
                    .json(&req),
            )
            .send()
            .await
            .context("dispatch task")?,
            "parse task export",
        )
        .await
    }

    pub async fn task_chain_op(&self, id: i64, req: &TaskChainOpRequest) -> Result<TaskExport> {
        self.handle_response(
            self.with_auth(
                self.inner
                    .http
                    .post(self.url(&format!("/tasks/{id}/chain-op")))
                    .json(&req),
            )
            .send()
            .await
            .context("record chain op")?,
            "parse task export",
        )
        .await
    }

    pub async fn task_close(&self, id: i64, req: &TaskCloseRequest) -> Result<TaskExport> {
        self.handle_response(
            self.with_auth(
                self.inner
                    .http
                    .post(self.url(&format!("/tasks/{id}/close")))
                    .json(&req),
            )
            .send()
            .await
            .context("close task")?,
            "parse task export",
        )
        .await
    }

    pub async fn task_reopen(&self, id: i64, req: &TaskForceRequest) -> Result<TaskExport> {
        self.handle_response(
            self.with_auth(
                self.inner
                    .http
                    .post(self.url(&format!("/tasks/{id}/reopen")))
                    .json(&req),
            )
            .send()
            .await
            .context("reopen task")?,
            "parse task export",
        )
        .await
    }

    pub async fn task_archive(&self, id: i64, req: &TaskForceRequest) -> Result<TaskExport> {
        self.handle_response(
            self.with_auth(
                self.inner
                    .http
                    .post(self.url(&format!("/tasks/{id}/archive")))
                    .json(&req),
            )
            .send()
            .await
            .context("archive task")?,
            "parse task export",
        )
        .await
    }
    /// Downloads the closed-task archive blob (canonical tar bytes).
    pub async fn task_fetch_archive(&self, id: i64) -> Result<Vec<u8>> {
        let resp = self
            .with_auth(
                self.inner
                    .http
                    .get(self.url(&format!("/tasks/{id}/archive"))),
            )
            .send()
            .await
            .context("fetch task archive")?;
        let resp = self.ensure_success(resp).await?;
        Ok(resp.bytes().await?.to_vec())
    }
}
