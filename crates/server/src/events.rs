//! Azioni sul mondo che i ganci (plugin e regole del server) possono annullare, e le modifiche ai blocchi che le compongono.

/// Una modifica a una cella: il blocco (hash dello stato) e se nel secondo strato (liquido) c'è acqua.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BlockChange {
    pub pos: [i32; 3],
    pub hash: i32,
    pub water: bool,
}

/// Un'azione di un giocatore sul mondo, mostrata ai ganci prima di applicarla.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(dead_code)] // la variante Interact la costruiranno le interazioni (porte, leve...)
pub(crate) enum WorldAction {
    /// Il giocatore rompe il blocco in `pos`, che prima era `old`.
    Break { pid: u64, pos: [i32; 3], old: i32 },
    /// Il giocatore piazza dei blocchi.
    Place { pid: u64, changes: Vec<BlockChange> },
    /// Il giocatore interagisce con un blocco (porta, leva...) e ne cambia lo stato.
    Interact { pid: u64, changes: Vec<BlockChange> },
}

/// Un gancio vede l'azione e risponde `true` per lasciarla passare, `false` per annullarla.
pub(crate) type Hook = Box<dyn FnMut(&WorldAction) -> bool + Send>;
