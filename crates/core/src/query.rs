use uuid::Uuid;

/// Opaque request identity. A new request invalidates all earlier batches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryToken {
    pub cell_id: Uuid,
    pub request_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum QueryStatus {
    #[default]
    Idle,
    Running,
    Completed,
    Cancelled,
    Failed(String),
}

#[derive(Debug)]
pub struct QueryState {
    cell_id: Uuid,
    active: Option<QueryToken>,
    pub status: QueryStatus,
    pub page: usize,
    pub page_size: usize,
    pub selected_row: Option<usize>,
    pub result_height: f32,
}

impl QueryState {
    pub fn new(cell_id: Uuid) -> Self {
        Self {
            cell_id,
            active: None,
            status: QueryStatus::Idle,
            page: 0,
            page_size: 256,
            selected_row: None,
            result_height: 240.0,
        }
    }

    pub fn begin(&mut self) -> QueryToken {
        let token = QueryToken {
            cell_id: self.cell_id,
            request_id: Uuid::new_v4(),
        };
        self.active = Some(token);
        self.status = QueryStatus::Running;
        self.page = 0;
        self.selected_row = None;
        token
    }

    pub fn accepts(&self, token: QueryToken) -> bool {
        self.active == Some(token) && self.status == QueryStatus::Running
    }

    pub fn complete(&mut self, token: QueryToken, result: Result<(), String>) -> bool {
        if !self.accepts(token) {
            return false;
        }
        self.active = None;
        self.status = match result {
            Ok(()) => QueryStatus::Completed,
            Err(message) => QueryStatus::Failed(message),
        };
        true
    }

    /// Invalidates responses immediately; the service must also cancel its I/O.
    pub fn cancel(&mut self) {
        if self.active.take().is_some() {
            self.status = QueryStatus::Cancelled;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rerun_and_cancel_discard_late_responses() {
        let mut state = QueryState::new(Uuid::new_v4());
        let old = state.begin();
        let current = state.begin();
        assert!(!state.complete(old, Ok(())));
        assert!(state.accepts(current));
        state.cancel();
        assert!(!state.complete(current, Ok(())));
        assert_eq!(state.status, QueryStatus::Cancelled);
    }

    #[test]
    fn cells_keep_independent_paging_and_selection() {
        let mut first = QueryState::new(Uuid::new_v4());
        let mut second = QueryState::new(Uuid::new_v4());
        first.page = 3;
        first.selected_row = Some(5);
        let token = second.begin();
        assert!(!first.accepts(token));
        assert_eq!((first.page, first.selected_row), (3, Some(5)));
        assert!(second.complete(token, Ok(())));
        assert!(!second.accepts(token));
    }
}
