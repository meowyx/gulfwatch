use gulfwatch_core::alert::AlertEvent;
use gulfwatch_core::transaction::Transaction;
use gulfwatch_core::AppState;
use tokio::sync::broadcast;

/// What the TUI is currently showing.
#[derive(PartialEq)]
pub enum View {
    /// Main dashboard with 3 panels.
    Dashboard,
    /// Detail view for a selected transaction.
    TransactionDetail(usize),
    /// Detail view for a selected alert.
    AlertDetail(usize),
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
        self.active_panel = if self.active_panel == 0 { 2 } else { self.active_panel - 1 };
        self.selected = 0;
    }

    pub fn scroll_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn scroll_down(&mut self) {
        let max = self.list_len().saturating_sub(1);
        if self.selected < max {
            self.selected += 1;
        }
    }

    /// Open detail view for the currently selected item.
    pub fn open_detail(&mut self) {
        match self.active_panel {
            0 if !self.transactions.is_empty() => {
                self.view = View::TransactionDetail(self.selected);
            }
            2 if !self.alerts.is_empty() => {
                self.view = View::AlertDetail(self.selected);
            }
            _ => {}
        }
    }

    /// Go back to dashboard view.
    pub fn close_detail(&mut self) {
        self.view = View::Dashboard;
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
