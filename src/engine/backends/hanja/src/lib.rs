use kime_engine_backend::{
    InputEngineMode,
    InputEngineModeResult::{self, Continue, Exit, ExitHandled},
    Key, KeyCode,
};

use kime_engine_candidate::client::Client;

/// Candidates shown per page. Must match the renderer's page size.
pub const PAGE_SIZE: usize = 10;

/// The candidate list and where the user is in it.
///
/// Pure data: no process, no I/O. The input method resolves keyboard selection
/// against this directly, so a keyboard pick never round trips through the
/// renderer and never depends on a focus change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CandidateState {
    entries: &'static [(&'static str, &'static str)],
    cursor: usize,
}

impl CandidateState {
    pub fn new(entries: &'static [(&'static str, &'static str)]) -> Option<Self> {
        if entries.is_empty() {
            None
        } else {
            Some(Self { entries, cursor: 0 })
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn page(&self) -> usize {
        self.cursor / PAGE_SIZE
    }

    pub fn page_count(&self) -> usize {
        self.len().div_ceil(PAGE_SIZE)
    }

    /// Number of candidates on the current page.
    pub fn page_len(&self) -> usize {
        let start = self.page() * PAGE_SIZE;
        (self.len() - start).min(PAGE_SIZE)
    }

    /// The hanja the cursor is on.
    pub fn selected(&self) -> &'static str {
        self.entries[self.cursor].0
    }

    pub fn key_at(&self, index: usize) -> Option<&'static str> {
        self.entries.get(index).map(|(key, _)| *key)
    }

    /// Absolute index of the `n`th candidate on the current page, if it exists.
    pub fn index_on_page(&self, n: usize) -> Option<usize> {
        if n < self.page_len() {
            Some(self.page() * PAGE_SIZE + n)
        } else {
            None
        }
    }

    /// Move the cursor within the whole list, clamping at both ends.
    pub fn move_cursor(&mut self, delta: isize) -> bool {
        let last = self.len() - 1;
        let next = self.cursor.saturating_add_signed(delta).min(last);

        if next == self.cursor {
            false
        } else {
            self.cursor = next;
            true
        }
    }

    /// Move whole pages, parking the cursor on the first candidate of the new
    /// page. Clamps at both ends.
    pub fn move_page(&mut self, delta: isize) -> bool {
        let last_page = self.page_count() - 1;
        let next_page = self.page().saturating_add_signed(delta).min(last_page);

        if next_page == self.page() {
            false
        } else {
            self.cursor = next_page * PAGE_SIZE;
            true
        }
    }
}

/// What a key press means while the candidate window is up.
enum Action {
    /// Pick the nth candidate on the current page.
    Select(usize),
    Page(isize),
    Cursor(isize),
    /// Dismiss, swallowing the key.
    Cancel,
    /// Dismiss and let the key reach the underlying engine.
    Passthrough,
}

fn action_for(key: Key) -> Action {
    use KeyCode::*;

    match key.code {
        One | NumOne => Action::Select(0),
        Two | NumTwo => Action::Select(1),
        Three | NumThree => Action::Select(2),
        Four | NumFour => Action::Select(3),
        Five | NumFive => Action::Select(4),
        Six | NumSix => Action::Select(5),
        Seven | NumSeven => Action::Select(6),
        Eight | NumEight => Action::Select(7),
        Nine | NumNine => Action::Select(8),
        Zero | NumZero => Action::Select(9),

        Left | H | PageUp => Action::Page(-1),
        Right | L | PageDown => Action::Page(1),

        Up | K => Action::Cursor(-1),
        Down | J => Action::Cursor(1),

        Home => Action::Cursor(isize::MIN),
        End => Action::Cursor(isize::MAX),

        Esc => Action::Cancel,

        _ => Action::Passthrough,
    }
}

#[derive(Debug)]
pub struct HanjaMode {
    state: Option<CandidateState>,
    client: Option<Client>,
}

impl Default for HanjaMode {
    fn default() -> Self {
        Self::new()
    }
}

impl HanjaMode {
    pub fn new() -> Self {
        Self {
            state: None,
            client: None,
        }
    }

    pub fn set_key(&mut self, key: &str) -> bool {
        let Some(entries) = kime_engine_dict::lookup(key) else {
            return false;
        };

        let Some(state) = CandidateState::new(entries) else {
            return false;
        };

        match Client::new(entries) {
            Ok(client) => {
                self.state = Some(state);
                self.client = Some(client);
                true
            }
            Err(err) => {
                // Without this the hanja key is swallowed
                // (`HotkeyResult::Consume`) and absolutely nothing happens, which
                // is indistinguishable from the key not being bound at all.
                log::error!(
                    "Failed to spawn {}: {err}. Hanja input needs that binary on PATH.",
                    kime_engine_candidate::client::CANDIDATE_PROCESS_NAME
                );
                false
            }
        }
    }

    /// fd that becomes readable when the renderer reports a mouse selection.
    pub fn ready_fd(&self) -> Option<std::os::unix::io::RawFd> {
        self.client.as_ref().map(Client::ready_fd)
    }

    /// Commit the highlighted candidate and end the session.
    fn commit_selected(&mut self, commit_buf: &mut String) {
        if let Some(state) = self.state.take() {
            commit_buf.push_str(state.selected());
        }

        if let Some(client) = self.client.take() {
            client.cancel();
        }
    }

    fn sync_renderer(&mut self) {
        let (Some(state), Some(client)) = (self.state.as_ref(), self.client.as_mut()) else {
            return;
        };

        if let Err(err) = client
            .set_page(state.page())
            .and_then(|()| client.set_highlight(state.cursor()))
        {
            log::warn!("Failed to update candidate window: {err}");
        }
    }
}

impl InputEngineMode for HanjaMode {
    type ConfigData = ();

    fn press_key(
        &mut self,
        _: &(),
        key: Key,
        commit_buf: &mut String,
    ) -> InputEngineModeResult<bool> {
        let Some(mut state) = self.state else {
            return Exit;
        };

        match action_for(key) {
            Action::Select(n) => match state.index_on_page(n) {
                Some(index) => {
                    if let Some(hanja) = state.key_at(index) {
                        commit_buf.push_str(hanja);
                    }
                    self.state = None;
                    if let Some(client) = self.client.take() {
                        client.cancel();
                    }
                    ExitHandled(true)
                }
                // Digit past the end of the page: swallow it rather than
                // dumping a stray number into the document.
                None => Continue(true),
            },
            Action::Page(delta) => {
                if state.move_page(delta) {
                    self.state = Some(state);
                    self.sync_renderer();
                }
                Continue(true)
            }
            Action::Cursor(delta) => {
                if state.move_cursor(delta) {
                    self.state = Some(state);
                    self.sync_renderer();
                }
                Continue(true)
            }
            Action::Cancel => {
                self.state = None;
                if let Some(client) = self.client.take() {
                    client.cancel();
                }
                ExitHandled(true)
            }
            Action::Passthrough => Exit,
        }
    }

    fn preedit_str(&self, buf: &mut String) {
        if let Some(state) = self.state.as_ref() {
            buf.push_str(state.selected());
        }
    }

    /// The `Commit` hotkey (Enter/Tab by default) lands here. Previously a no-op,
    /// which is why Enter did nothing in hanja mode.
    fn clear_preedit(&mut self, commit_buf: &mut String) -> InputEngineModeResult<()> {
        self.commit_selected(commit_buf);
        ExitHandled(())
    }

    fn reset(&mut self) -> InputEngineModeResult<()> {
        self.state = None;

        if let Some(client) = self.client.take() {
            client.cancel();
        }

        ExitHandled(())
    }

    fn has_preedit(&self) -> bool {
        true
    }

    fn check_ready(&self) -> bool {
        self.client.as_ref().map(Client::is_ready).unwrap_or(true)
    }

    /// Mouse selection: the renderer replied with an index.
    fn end_ready(&mut self, commit_buf: &mut String) -> InputEngineModeResult<()> {
        let state = self.state.take();

        match self.client.take() {
            Some(client) => match client.close() {
                Ok(Some(index)) => match state.and_then(|state| state.key_at(index)) {
                    Some(hanja) => {
                        commit_buf.push_str(hanja);
                        ExitHandled(())
                    }
                    None => {
                        log::warn!("Candidate window selected out of range index {index}");
                        Exit
                    }
                },
                Ok(None) => Exit,
                Err(err) => {
                    log::error!("Candidate window failed: {err}");
                    Exit
                }
            },
            None => Exit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries() -> &'static [(&'static str, &'static str)] {
        kime_engine_dict::lookup("가").expect("가 is in the hanja dictionary")
    }

    fn state_of(n: usize) -> CandidateState {
        // Leak a slice of a known length so page arithmetic is exercised
        // deterministically regardless of dictionary contents.
        let entries: &'static [(&'static str, &'static str)] =
            Box::leak(vec![("漢", "한수 한"); n].into_boxed_slice());
        CandidateState::new(entries).unwrap()
    }

    #[test]
    fn rejects_empty() {
        assert!(CandidateState::new(&[]).is_none());
    }

    #[test]
    fn real_dictionary_entry_is_usable() {
        let state = CandidateState::new(entries()).unwrap();
        assert!(!state.is_empty());
        assert_eq!(state.page(), 0);
        assert_eq!(state.cursor(), 0);
        assert_eq!(state.selected(), state.key_at(0).unwrap());
    }

    #[test]
    fn page_count_rounds_up() {
        assert_eq!(state_of(1).page_count(), 1);
        assert_eq!(state_of(10).page_count(), 1);
        assert_eq!(state_of(11).page_count(), 2);
        assert_eq!(state_of(20).page_count(), 2);
        assert_eq!(state_of(21).page_count(), 3);
    }

    #[test]
    fn page_len_covers_the_short_last_page() {
        let mut state = state_of(23);
        assert_eq!(state.page_len(), 10);
        assert!(state.move_page(2));
        assert_eq!(state.page(), 2);
        assert_eq!(state.page_len(), 3);
    }

    #[test]
    fn index_on_page_is_bounded_by_the_page() {
        let mut state = state_of(23);
        assert_eq!(state.index_on_page(0), Some(0));
        assert_eq!(state.index_on_page(9), Some(9));
        assert_eq!(state.index_on_page(10), None);

        state.move_page(2);
        assert_eq!(state.index_on_page(0), Some(20));
        assert_eq!(state.index_on_page(2), Some(22));
        // Page 2 only holds 3 candidates.
        assert_eq!(state.index_on_page(3), None);
    }

    #[test]
    fn cursor_clamps_at_both_ends() {
        let mut state = state_of(5);
        assert!(!state.move_cursor(-1));
        assert_eq!(state.cursor(), 0);

        assert!(state.move_cursor(4));
        assert_eq!(state.cursor(), 4);
        assert!(!state.move_cursor(1));
        assert_eq!(state.cursor(), 4);

        assert!(state.move_cursor(isize::MIN));
        assert_eq!(state.cursor(), 0);
        assert!(state.move_cursor(isize::MAX));
        assert_eq!(state.cursor(), 4);
    }

    #[test]
    fn paging_parks_cursor_on_the_page_start() {
        let mut state = state_of(23);
        assert!(state.move_page(1));
        assert_eq!(state.cursor(), 10);
        assert_eq!(state.page(), 1);

        assert!(state.move_page(-1));
        assert_eq!(state.cursor(), 0);

        // Already on the first page.
        assert!(!state.move_page(-1));
        assert_eq!(state.cursor(), 0);
    }

    #[test]
    fn paging_clamps_to_the_last_page() {
        let mut state = state_of(23);
        assert!(state.move_page(99));
        assert_eq!(state.page(), 2);
        assert!(!state.move_page(1));
        assert_eq!(state.page(), 2);
    }

    #[test]
    fn single_page_has_no_paging() {
        let mut state = state_of(4);
        assert!(!state.move_page(1));
        assert!(!state.move_page(-1));
        assert_eq!(state.page(), 0);
        assert_eq!(state.page_len(), 4);
    }

    #[test]
    fn digits_map_to_page_offsets() {
        use KeyCode::*;

        for (code, expected) in [
            (One, 0),
            (Five, 4),
            (Nine, 8),
            (Zero, 9),
            (NumOne, 0),
            (NumZero, 9),
        ] {
            match action_for(Key::normal(code)) {
                Action::Select(n) => assert_eq!(n, expected, "{code:?}"),
                _ => panic!("{code:?} should select"),
            }
        }
    }

    #[test]
    fn navigation_keys_map_to_paging_and_cursor() {
        use KeyCode::*;

        assert!(matches!(
            action_for(Key::normal(Right)),
            Action::Page(d) if d > 0
        ));
        assert!(matches!(
            action_for(Key::normal(Left)),
            Action::Page(d) if d < 0
        ));
        assert!(matches!(
            action_for(Key::normal(Down)),
            Action::Cursor(d) if d > 0
        ));
        assert!(matches!(action_for(Key::normal(Esc)), Action::Cancel));
        // Ordinary typing dismisses and reaches the underlying engine.
        assert!(matches!(action_for(Key::normal(A)), Action::Passthrough));
    }
}
