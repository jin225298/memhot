use super::*;

struct Fixture {
    app: App,
    user: String,
}
impl Fixture {
    fn new() -> Self {
        let app = App::open(std::env::temp_dir().join(format!("reading-commit-{}", crate::id())))
            .unwrap();
        let (user, _) = crate::auth::create_pair(&app, "synthetic", None).unwrap();
        app.store.put(&user,"report",&json!({"id":"r","kind":"research","createdAt":crate::now(),"title":"synthetic report","sources":[{"id":"e","url":"https://example.com/original","title":"synthetic source","excerpt":"short excerpt","retrievalKind":"page"}]}),None).unwrap();
        app.store.put(&user,"raw",&json!({"id":crate::hash(b"https://example.com/original"),"url":"https://example.com/original","createdAt":crate::now(),"text":"Source material from the saved page.","expiresAt":chrono::Utc::now().timestamp()+3600}),None).unwrap();
        Self { app, user }
    }
    async fn queue(&self) -> (Snapshot, Job) {
        let (status, _) = queue_translation(
            State(self.app.clone()),
            User(self.user.clone()),
            Path(("r".into(), "e".into())),
        )
        .await
        .unwrap();
        assert_eq!(status, StatusCode::ACCEPTED);
        let job = self.app.store.claim().unwrap().unwrap();
        let snapshot = materials::load(&self.app, &self.user, "r", "e").unwrap();
        (snapshot, job)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.app.data_dir.as_ref());
    }
}

#[tokio::test]
async fn untranslated_english_is_never_published_as_a_completed_translation() {
    let f = Fixture::new();
    let (snapshot, job) = f.queue().await;
    let echoed = snapshot.text().replace(' ', "  ");
    assert_eq!(
        commit(&f.app, &job, &snapshot, &echoed).unwrap_err().code,
        "translation_unchanged"
    );
    assert!(f
        .app
        .store
        .list_all(&f.user, "material_translation")
        .unwrap()
        .is_empty());
    assert_ne!(f.app.store.jobs(&f.user).unwrap()[0]["status"], "completed");
    assert!(validate_translation("保留原有中文材料", "保留原有中文材料").is_ok());
    assert!(validate_translation("Pi Agent SDK 1.1.0", "Pi Agent SDK 1.1.0").is_ok());
    commit(&f.app, &job, &snapshot, "保存页面中的来源材料。").unwrap();
    assert_eq!(
        state(&f.app, &f.user, &snapshot).unwrap().unwrap()["status"],
        "ready"
    );
}

#[tokio::test]
async fn late_translation_cannot_commit_after_report_delete_edit_epoch_expiry_or_lease_loss() {
    for change in ["report-delete", "report-edit", "epoch", "expiry", "lease"] {
        let f = Fixture::new();
        let (snapshot, job) = f.queue().await;
        {
            let conn = f.app.store.conn().unwrap();
            match change {
                "report-delete" => {
                    conn.execute(
                        "DELETE FROM records WHERE user_id=? AND kind='report' AND id='r'",
                        [&f.user],
                    )
                    .unwrap();
                }
                "report-edit" => {
                    conn.execute("UPDATE records SET data=json_set(data,'$.title','new title') WHERE user_id=? AND kind='report' AND id='r'",[&f.user]).unwrap();
                }
                "epoch" => {
                    conn.execute("UPDATE users SET epoch=epoch+1 WHERE id=?", [&f.user])
                        .unwrap();
                }
                "expiry" => {
                    conn.execute("UPDATE records SET data=json_set(data,'$.expiresAt',?) WHERE user_id=? AND kind='raw'",params![chrono::Utc::now().timestamp()-1,f.user]).unwrap();
                }
                "lease" => {
                    conn.execute("UPDATE jobs SET attempt=attempt+1 WHERE id=?", [&job.id])
                        .unwrap();
                }
                _ => unreachable!(),
            }
        }
        assert!(
            commit(&f.app, &job, &snapshot, "已经生成的旧译文").is_err(),
            "late {change} must win over SDK completion"
        );
        assert!(f
            .app
            .store
            .list_all(&f.user, "material_translation")
            .unwrap()
            .is_empty());
        assert_ne!(f.app.store.jobs(&f.user).unwrap()[0]["status"], "completed");
    }
}

#[tokio::test]
async fn completed_translation_stays_idempotent_without_old_jobs_and_expired_text_is_hidden() {
    let f = Fixture::new();
    let (snapshot, job) = f.queue().await;
    commit(&f.app, &job, &snapshot, "已保存页面的中文译文。").unwrap();
    assert_eq!(f.app.store.jobs(&f.user).unwrap()[0]["status"], "completed");
    let ready = state(&f.app, &f.user, &snapshot).unwrap().unwrap();
    assert_eq!(ready["status"], "ready");
    assert_eq!(ready["text"], "已保存页面的中文译文。");
    {
        let conn = f.app.store.conn().unwrap();
        conn.execute("DELETE FROM jobs WHERE user_id=?", [&f.user])
            .unwrap();
        conn.execute(
            "UPDATE billing_wallets SET included=0,topup=0 WHERE user_id=?",
            [&f.user],
        )
        .unwrap();
    }
    let (_, Json(queued)) = queue_translation(
        State(f.app.clone()),
        User(f.user.clone()),
        Path(("r".into(), "e".into())),
    )
    .await
    .unwrap();
    assert_eq!(queued["jobId"], job.id);
    assert_eq!(queued["status"], "completed");
    assert!(f.app.store.jobs(&f.user).unwrap().is_empty());
    f.app.store.conn().unwrap().execute("UPDATE records SET data=json_set(data,'$.expiresAt',?) WHERE user_id=? AND kind='material_translation'",params![chrono::Utc::now().timestamp()-1,f.user]).unwrap();
    assert!(
        state(&f.app, &f.user, &snapshot).unwrap().is_none(),
        "expired full-page translations must never be served"
    );
}

#[tokio::test]
async fn raw_content_replaced_during_translation_is_rejected_even_before_its_ttl() {
    let f = Fixture::new();
    let (snapshot, job) = f.queue().await;
    f.app.store.conn().unwrap().execute("UPDATE records SET data=json_set(data,'$.text','A different page revision') WHERE user_id=? AND kind='raw'",[&f.user]).unwrap();
    assert_eq!(
        commit(&f.app, &job, &snapshot, "旧译文").unwrap_err().code,
        "translation_material_expired"
    );
    assert!(f
        .app
        .store
        .list_all(&f.user, "material_translation")
        .unwrap()
        .is_empty());
}
