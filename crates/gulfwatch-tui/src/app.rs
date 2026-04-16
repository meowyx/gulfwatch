use gulfwatch_core::alert::AlertEvent;
use gulfwatch_core::transaction::Transaction;
use gulfwatch_core::AppState;
use tokio::sync::broadcast;

pub enum View {
    Dashboard,
    TransactionDetail(Box<Transaction>),
    AlertDetail(Box<AlertEvent>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailTab {
    Overview,
    Instructions,
    Logs,
    Accounts,
    Diff,
    Errors,
}

impl DetailTab {
    pub const ALL: [DetailTab; 6] = [
        DetailTab::Overview,
        DetailTab::Instructions,
        DetailTab::Logs,
        DetailTab::Accounts,
        DetailTab::Diff,
        DetailTab::Errors,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DetailTab::Overview => "Overview",
            DetailTab::Instructions => "Instructions",
            DetailTab::Logs => "Logs",
            DetailTab::Accounts => "Accounts",
            DetailTab::Diff => "Diff",
            DetailTab::Errors => "Errors",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }
}

pub struct App {
    pub state: AppState,
    tx_rx: broadcast::Receiver<Transaction>,
    alert_rx: broadcast::Receiver<AlertEvent>,

    pub transactions: Vec<Transaction>,
    pub alerts: Vec<AlertEvent>,
    pub programs: Vec<String>,
    pub active_panel: usize,
    pub selected: usize,
    // None = "All" merged view; Some(i) filters to programs[i].
    pub selected_program: Option<usize>,
    pub view: View,
    max_feed_size: usize,
    pub detail_scroll: u16,
    pub detail_tab: DetailTab,
    pub metrics_scroll: u16,
}

pub const PANEL_COUNT: usize = 4;
pub const PANEL_SIDEBAR: usize = 0;
pub const PANEL_TRANSACTIONS: usize = 1;
pub const PANEL_METRICS: usize = 2;
pub const PANEL_ALERTS: usize = 3;

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
            programs: Vec::new(),
            active_panel: PANEL_TRANSACTIONS,
            selected: 0,
            selected_program: None,
            view: View::Dashboard,
            max_feed_size: 500,
            detail_scroll: 0,
            detail_tab: DetailTab::Overview,
            metrics_scroll: 0,
        }
    }

    pub fn refresh_programs(&mut self) {
        if let Ok(programs) = self.state.monitored_programs.try_read() {
            self.programs = programs.clone();
        }
        if let Some(idx) = self.selected_program {
            if idx >= self.programs.len() {
                self.selected_program = if self.programs.is_empty() {
                    None
                } else {
                    Some(self.programs.len() - 1)
                };
            }
        }
    }

    pub fn focused_program_id(&self) -> Option<&str> {
        self.selected_program
            .and_then(|i| self.programs.get(i).map(|s| s.as_str()))
    }

    pub fn focused_program_label(&self) -> String {
        match self.focused_program_id() {
            Some(pid) => short_program_id(pid),
            None => "All".to_string(),
        }
    }

    pub fn select_program(&mut self, idx: Option<usize>) {
        match idx {
            None => self.selected_program = None,
            Some(i) if i < self.programs.len() => self.selected_program = Some(i),
            _ => {}
        }
    }

    pub fn filtered_transactions(&self) -> impl Iterator<Item = &Transaction> {
        let focused = self.focused_program_id().map(|s| s.to_string());
        self.transactions.iter().filter(move |tx| match &focused {
            Some(pid) => tx.program_id == *pid,
            None => true,
        })
    }

    pub fn filtered_alerts(&self) -> impl Iterator<Item = &AlertEvent> {
        let focused = self.focused_program_id().map(|s| s.to_string());
        self.alerts.iter().filter(move |a| match &focused {
            Some(pid) => a.program_id == *pid,
            None => true,
        })
    }

    pub fn program_has_recent_alert(&self, program_id: &str) -> bool {
        self.alerts.iter().any(|a| a.program_id == program_id)
    }

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

        self.refresh_programs();
    }

    pub fn next_panel(&mut self) {
        self.active_panel = (self.active_panel + 1) % PANEL_COUNT;
        self.selected = 0;
    }

    pub fn prev_panel(&mut self) {
        self.active_panel = if self.active_panel == 0 {
            PANEL_COUNT - 1
        } else {
            self.active_panel - 1
        };
        self.selected = 0;
    }

    pub fn scroll_up(&mut self) {
        if !matches!(self.view, View::Dashboard) {
            self.detail_scroll = self.detail_scroll.saturating_sub(1);
            return;
        }
        match self.active_panel {
            PANEL_METRICS => {
                self.metrics_scroll = self.metrics_scroll.saturating_sub(1);
            }
            _ => {
                self.selected = self.selected.saturating_sub(1);
            }
        }
    }

    pub fn scroll_down(&mut self) {
        if !matches!(self.view, View::Dashboard) {
            self.detail_scroll = self.detail_scroll.saturating_add(1);
            return;
        }
        match self.active_panel {
            PANEL_METRICS => {
                self.metrics_scroll = self.metrics_scroll.saturating_add(1);
            }
            _ => {
                let max = self.list_len().saturating_sub(1);
                if self.selected < max {
                    self.selected += 1;
                }
            }
        }
    }

    pub fn open_detail(&mut self) {
        match self.active_panel {
            PANEL_SIDEBAR => {
                if self.selected == 0 {
                    self.selected_program = None;
                } else {
                    self.select_program(Some(self.selected - 1));
                }
            }
            PANEL_TRANSACTIONS => {
                let picked = self.filtered_transactions().nth(self.selected).cloned();
                if let Some(tx) = picked {
                    self.view = View::TransactionDetail(Box::new(tx));
                    self.detail_scroll = 0;
                    self.detail_tab = DetailTab::Overview;
                }
            }
            PANEL_ALERTS => {
                let picked = self.filtered_alerts().nth(self.selected).cloned();
                if let Some(alert) = picked {
                    self.view = View::AlertDetail(Box::new(alert));
                    self.detail_scroll = 0;
                }
            }
            _ => {}
        }
    }

    pub fn close_detail(&mut self) {
        self.view = View::Dashboard;
        self.detail_scroll = 0;
        self.detail_tab = DetailTab::Overview;
    }

    pub fn next_detail_tab(&mut self) {
        if !matches!(self.view, View::TransactionDetail(_)) {
            return;
        }
        let i = self.detail_tab.index();
        self.detail_tab = DetailTab::ALL[(i + 1) % DetailTab::ALL.len()];
        self.detail_scroll = 0;
    }

    pub fn prev_detail_tab(&mut self) {
        if !matches!(self.view, View::TransactionDetail(_)) {
            return;
        }
        let i = self.detail_tab.index();
        let n = DetailTab::ALL.len();
        self.detail_tab = DetailTab::ALL[(i + n - 1) % n];
        self.detail_scroll = 0;
    }

    fn list_len(&self) -> usize {
        match self.active_panel {
            PANEL_SIDEBAR => self.programs.len() + 1,
            PANEL_TRANSACTIONS => self.filtered_transactions().count(),
            PANEL_ALERTS => self.filtered_alerts().count(),
            _ => 0,
        }
    }
}

pub fn short_program_id(pid: &str) -> String {
    if pid.len() <= 12 {
        pid.to_string()
    } else {
        format!("{}…{}", &pid[..4], &pid[pid.len() - 4..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use gulfwatch_core::transaction::Transaction;

    fn make_tx() -> Transaction {
        tx_for_program("prog")
    }

    fn tx_for_program(program_id: &str) -> Transaction {
        Transaction {
            signature: format!("sig_{program_id}"),
            program_id: program_id.to_string(),
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
            logs: vec![],
            balance_diff: None,
            tx_error: None,
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
    async fn selecting_program_filters_transactions_and_alerts() {
        let (state, _rx) = AppState::new(16, 10);
        state.add_program("alpha".to_string()).await;
        state.add_program("beta".to_string()).await;

        let mut app = App::new(state);
        app.refresh_programs();
        app.transactions.push(tx_for_program("alpha"));
        app.transactions.push(tx_for_program("beta"));
        app.transactions.push(tx_for_program("alpha"));
        app.alerts.push(AlertEvent {
            rule_id: "r1".to_string(),
            rule_name: "rule".to_string(),
            program_id: "alpha".to_string(),
            metric: "m".to_string(),
            value: 1.0,
            threshold: 0.0,
            fired_at: Utc::now(),
        });
        app.alerts.push(AlertEvent {
            rule_id: "r2".to_string(),
            rule_name: "rule".to_string(),
            program_id: "beta".to_string(),
            metric: "m".to_string(),
            value: 1.0,
            threshold: 0.0,
            fired_at: Utc::now(),
        });

        assert_eq!(app.filtered_transactions().count(), 3);
        assert_eq!(app.filtered_alerts().count(), 2);
        assert_eq!(app.focused_program_label(), "All");

        app.select_program(Some(0));
        assert_eq!(app.focused_program_id(), Some("alpha"));
        assert_eq!(app.filtered_transactions().count(), 2);
        assert_eq!(app.filtered_alerts().count(), 1);

        app.select_program(Some(1));
        assert_eq!(app.focused_program_id(), Some("beta"));
        assert_eq!(app.filtered_transactions().count(), 1);
        assert_eq!(app.filtered_alerts().count(), 1);

        app.select_program(Some(99));
        assert_eq!(app.focused_program_id(), Some("beta"));

        app.select_program(None);
        assert_eq!(app.focused_program_id(), None);
        assert_eq!(app.filtered_transactions().count(), 3);
    }

    #[tokio::test]
    async fn detail_tab_cycles_forward_and_back() {
        let (state, _rx) = AppState::new(16, 10);
        let mut app = App::new(state);
        app.transactions.push(make_tx());
        app.open_detail();

        assert_eq!(app.detail_tab, DetailTab::Overview);
        for expected in [
            DetailTab::Instructions,
            DetailTab::Logs,
            DetailTab::Accounts,
            DetailTab::Diff,
            DetailTab::Errors,
        ] {
            app.next_detail_tab();
            assert_eq!(app.detail_tab, expected);
        }
        app.next_detail_tab();
        assert_eq!(app.detail_tab, DetailTab::Overview, "wraps around");

        app.prev_detail_tab();
        assert_eq!(app.detail_tab, DetailTab::Errors);
    }

    #[tokio::test]
    async fn detail_tab_navigation_resets_scroll() {
        let (state, _rx) = AppState::new(16, 10);
        let mut app = App::new(state);
        app.transactions.push(make_tx());
        app.open_detail();
        app.scroll_down();
        app.scroll_down();
        assert_eq!(app.detail_scroll, 2);

        app.next_detail_tab();
        assert_eq!(app.detail_scroll, 0);
    }

    #[tokio::test]
    async fn detail_tab_navigation_noop_outside_tx_detail() {
        let (state, _rx) = AppState::new(16, 10);
        let mut app = App::new(state);
        app.next_detail_tab();
        assert_eq!(app.detail_tab, DetailTab::Overview, "no-op on dashboard");
    }

    #[tokio::test]
    async fn closing_detail_resets_tab() {
        let (state, _rx) = AppState::new(16, 10);
        let mut app = App::new(state);
        app.transactions.push(make_tx());
        app.open_detail();
        app.next_detail_tab();
        app.next_detail_tab();
        assert_eq!(app.detail_tab, DetailTab::Logs);

        app.close_detail();
        assert_eq!(app.detail_tab, DetailTab::Overview);
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
