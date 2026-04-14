use gulfwatch_core::alert::AlertEvent;
use gulfwatch_core::transaction::Transaction;
use gulfwatch_core::AppState;
use tokio::sync::broadcast;

/// What the TUI is currently showing.
pub enum View {
    /// Main dashboard with 3 panels.
    Dashboard,
    /// Detail view for a snapshot of the selected transaction.
    /// Holds its own copy so the live feed can't shift it underneath.
    TransactionDetail(Box<Transaction>),
    /// Detail view for a snapshot of the selected alert.
    AlertDetail(Box<AlertEvent>),
}

/// Application state for the TUI.
pub struct App {
    pub state: AppState,
    tx_rx: broadcast::Receiver<Transaction>,
    alert_rx: broadcast::Receiver<AlertEvent>,

    /// Recent transactions displayed in the feed panel.
    pub transactions: Vec<Transaction>,
    /// Recent alert events.
    pub alerts: Vec<AlertEvent>,
    /// Which panel is active (0=transactions, 1=metrics, 2=alerts).
    pub active_panel: usize,
    /// Selected row index within the active panel.
    pub selected: usize,
    /// Current view (dashboard or detail).
    pub view: View,
    /// Max transactions to keep in the feed.
    max_feed_size: usize,
    /// Vertical scroll offset for detail views.
    pub detail_scroll: u16,
}

impl App {
    pub fn new(state: AppState) -> Self {
        let tx_rx = state.tx_broadcast.subscribe();
        let alert_rx = state.alert_broadcast.subscribe();

        Self {
            state,
            tx_rx,
            alert_rx,
            transactions: Vec::new(),
            alerts: Vec::new(),
            active_panel: 0,
            selected: 0,
            view: View::Dashboard,
            max_feed_size: 500,
            detail_scroll: 0,
        }
    }

    /// Non-blocking poll for new transactions and alerts from broadcast channels.
    pub fn poll_updates(&mut self) {
        loop {
            match self.tx_rx.try_recv() {
                Ok(tx) => {
                    self.transactions.insert(0, tx);
                    if self.transactions.len() > self.max_feed_size {
                        self.transactions.pop();
                    }
                }
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }

        loop {
            match self.alert_rx.try_recv() {
                Ok(alert) => {
                    self.alerts.insert(0, alert);
                    if self.alerts.len() > 50 {
                        self.alerts.pop();
                    }
                }
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    }

    pub fn next_panel(&mut self) {
        self.active_panel = (self.active_panel + 1) % 3;
        self.selected = 0;
    }

    pub fn prev_panel(&mut self) {
        self.active_panel = if self.active_panel == 0 {
            2
        } else {
            self.active_panel - 1
        };
        self.selected = 0;
    }

    pub fn scroll_up(&mut self) {
        if matches!(self.view, View::Dashboard) {
            self.selected = self.selected.saturating_sub(1);
        } else {
            self.detail_scroll = self.detail_scroll.saturating_sub(1);
        }
    }

    pub fn scroll_down(&mut self) {
        if matches!(self.view, View::Dashboard) {
            let max = self.list_len().saturating_sub(1);
            if self.selected < max {
                self.selected += 1;
            }
        } else {
            self.detail_scroll = self.detail_scroll.saturating_add(1);
        }
    }

    /// Open detail view for the currently selected item.
    /// Snapshots the item by clone, so the live feed sliding under us
    /// can't change what the detail view is showing.
    pub fn open_detail(&mut self) {
        match self.active_panel {
            0 => {
                if let Some(tx) = self.transactions.get(self.selected) {
                    self.view = View::TransactionDetail(Box::new(tx.clone()));
                    self.detail_scroll = 0;
                }
            }
            2 => {
                if let Some(alert) = self.alerts.get(self.selected) {
                    self.view = View::AlertDetail(Box::new(alert.clone()));
                    self.detail_scroll = 0;
                }
            }
            _ => {}
        }
    }

    /// Go back to dashboard view.
    pub fn close_detail(&mut self) {
        self.view = View::Dashboard;
        self.detail_scroll = 0;
    }

    /// Number of items in the currently active panel's list.
    fn list_len(&self) -> usize {
        match self.active_panel {
            0 => self.transactions.len(),
            2 => self.alerts.len(),
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use gulfwatch_core::transaction::Transaction;

    fn make_tx() -> Transaction {
        Transaction {
            signature: "sig".to_string(),
            program_id: "prog".to_string(),
            block_slot: 1,
            timestamp: Utc::now(),
            success: true,
            instruction_type: Some("transfer".to_string()),
            accounts: vec!["a".to_string(), "b".to_string()],
            fee_lamports: 5000,
            compute_units: 1234,
            instructions: vec![],
            cu_profile: None,
            classification: None,
            classification_debug: None,
        }
    }

    #[tokio::test]
    async fn detail_scroll_moves_only_in_detail_view() {
        let (state, _rx) = AppState::new(16, 10);
        let mut app = App::new(state);
        app.transactions.push(make_tx());

        app.scroll_down();
        assert_eq!(app.detail_scroll, 0);

        app.open_detail();
        app.scroll_down();
        app.scroll_down();
        assert_eq!(app.detail_scroll, 2);

        app.close_detail();
        assert_eq!(app.detail_scroll, 0);
    }

    #[tokio::test]
    async fn opening_detail_resets_scroll() {
        let (state, _rx) = AppState::new(16, 10);
        let mut app = App::new(state);
        app.transactions.push(make_tx());

        app.open_detail();
        app.scroll_down();
        app.scroll_down();
        assert_eq!(app.detail_scroll, 2);

        app.close_detail();
        app.open_detail();
        assert_eq!(app.detail_scroll, 0);
    }
}
