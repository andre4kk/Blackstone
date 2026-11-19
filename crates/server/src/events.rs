//! Azioni sul mondo che i ganci (plugin e regole del server) possono annullare, e le modifiche ai blocchi che le compongono.
use crate::{inventory::Item, survival::DamageCause};

/// Una modifica a una cella: il blocco (hash dello stato) e se nel secondo strato (liquido) c'è acqua.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BlockChange {
    pub pos: [i32; 3],
    pub hash: i32,
    pub water: bool,
}

/// Un'azione di un giocatore sul mondo, mostrata ai ganci prima di applicarla.
// niente `Eq`: `Damage` porta un `f32`
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum WorldAction {
    /// Il giocatore butta `item` (una casella dell'inventario, in tutto o in parte): diventa un oggetto a terra.
    DropItem { pid: u64, item: Item },
    /// Il giocatore sta per raccogliere `item` (l'intera pila a terra, o la parte che ci sta) dall'entità `entity`.
    PickupItem { pid: u64, entity: u64, item: Item },
    /// Il giocatore rompe il blocco in `pos`, che prima era `old`.
    Break { pid: u64, pos: [i32; 3], old: i32 },
    /// Il giocatore piazza dei blocchi.
    Place { pid: u64, changes: Vec<BlockChange> },
    /// Il giocatore interagisce con un blocco (porta, leva...) e ne cambia lo stato.
    Interact { pid: u64, changes: Vec<BlockChange> },
    /// Il giocatore sta per subire `amount` di danno (quello che resta dopo l'invulnerabilità, come l'evento `HandleHurt` di Dragonfly).
    Damage { pid: u64, amount: f32, cause: DamageCause },
    /// Il giocatore è appena morto (vita a 0). I ganci possono solo osservare: la loro risposta è ignorata (la morte non si annulla).
    Death { pid: u64 },
}

/// Un gancio vede l'azione e risponde `true` per lasciarla passare, `false` per annullarla.
pub(crate) type Hook = Box<dyn FnMut(&WorldAction) -> bool + Send>;
