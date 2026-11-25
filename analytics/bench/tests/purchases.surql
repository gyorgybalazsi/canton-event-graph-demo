-- Two ticket purchases as the syncer writes them for the demo's model
-- (daml/daml/Main.daml): the bank issues cash, the organizer offers a
-- ticket, the buyer accepts. Only the fields the ticket plugins read, and
-- what they depend on, are filled in.
LET $offer = { package_id: 'pkg', module_name: 'Main', entity_name: 'TicketOffer' };
LET $cash = { package_id: 'pkg', module_name: 'Main', entity_name: 'Cash' };

UPSERT created:⟨00a1⟩ CONTENT { contract_id: '00a1', name: 'Cash@1', template_name: 'Main.Cash', template: $cash, offset: 1, node_id: 0,
    signatories: ['ScroogeBank::1220aa'], created_at: d'2026-11-02T09:00:00Z',
    create_arguments: { issuer: 'ScroogeBank::1220aa', owner: 'Alice::1220bb', amount: 25.0dec } };
UPSERT created:⟨00a2⟩ CONTENT { contract_id: '00a2', name: 'TicketOffer@2', template_name: 'Main.TicketOffer', template: $offer, offset: 2, node_id: 0,
    signatories: ['TicketWizard::1220cc'], created_at: d'2026-11-02T09:01:00Z',
    create_arguments: { organizer: 'TicketWizard::1220cc', buyer: 'Alice::1220bb', price: 25.0dec } };
UPSERT exercised:[3, 0] CONTENT { name: 'Accept@3', choice_name: 'Accept', template: $offer, target_contract_id: '00a2',
    acting_parties: ['Alice::1220bb'], consuming: true, offset: 3, node_id: 0,
    transaction_effective_at: d'2026-11-02T09:02:00Z', choice_argument: { cashId: '00a1' } };
UPSERT exercised:[3, 1] CONTENT { name: 'Transfer@3', choice_name: 'Transfer', template: $cash, target_contract_id: '00a1',
    acting_parties: ['Alice::1220bb'], consuming: false, offset: 3, node_id: 1,
    transaction_effective_at: d'2026-11-02T09:02:00Z', choice_argument: { newOwner: 'TicketWizard::1220cc' } };

UPSERT created:⟨00b1⟩ CONTENT { contract_id: '00b1', name: 'TicketOffer@4', template_name: 'Main.TicketOffer', template: $offer, offset: 4, node_id: 0,
    signatories: ['TicketWizard::1220cc'], created_at: d'2026-11-02T10:00:00Z',
    create_arguments: { organizer: 'TicketWizard::1220cc', buyer: 'Bob::1220dd', price: 40.5dec } };
UPSERT exercised:[5, 0] CONTENT { name: 'Accept@5', choice_name: 'Accept', template: $offer, target_contract_id: '00b1',
    acting_parties: ['Bob::1220dd'], consuming: true, offset: 5, node_id: 0,
    transaction_effective_at: d'2026-11-02T10:05:00Z', choice_argument: { cashId: '00b0' } };

UPSERT sync_state:stream CONTENT { last_offset: 5 };
