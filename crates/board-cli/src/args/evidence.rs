use clap::Subcommand;

#[derive(Subcommand)]
pub(crate) enum EvidenceCmd {
    /// Idempotently attach machine evidence to a card.
    Put {
        card_id: i64,
        #[arg(long)]
        producer: String,
        #[arg(long)]
        dedupe_key: String,
        #[arg(long)]
        schema: String,
        #[arg(long)]
        payload_json: String,
    },
    /// Read evidence by its stable producer key.
    Get {
        card_id: i64,
        #[arg(long)]
        producer: String,
        #[arg(long)]
        dedupe_key: String,
    },
}

#[derive(Subcommand)]
pub(crate) enum DecisionCmd {
    /// Open or replay an exact blocked-question occurrence.
    Open {
        board_id: i64,
        card_id: i64,
        run_id: i64,
        #[arg(long)]
        question_hash: String,
        #[arg(long)]
        target_json: String,
    },
    /// Inspect one durable blocked-question occurrence.
    Get { decision_id: String },
    /// Adopt an authenticated exact-answer receipt.
    Receipt {
        decision_id: String,
        #[arg(long)]
        delivery_id: String,
        #[arg(long)]
        target_fingerprint: String,
        #[arg(long)]
        answer_hash: String,
        #[arg(long, default_value = "accepted", value_parser = ["accepted"])]
        status: String,
        #[arg(long)]
        receipt_signature: String,
    },
}
