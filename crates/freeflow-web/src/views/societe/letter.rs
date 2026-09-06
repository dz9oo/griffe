//! Lettres de La société (lot 51) : accueil, Te payer, impôts, clore, relevé, identité.

use freeflow_core::app::AppError;
use freeflow_core::domain::{Money, VatRegime, format_date_fr};
use freeflow_core::society::{
    BeatKind, BeatWhen, ClosingBeat, ClosingStory, ConversationBar, DepositPlace, DividendClosed,
    DividendDoor, Duty, IdentityCard, IdentityShort, Landscape, PayYourself, SocietyHome,
    StatementMove, StatementReading, closing_story, pay_yourself, society_duties, society_home,
    society_identity, statement_moves,
};
use freeflow_core::store::Store;
use maud::{Markup, PreEscaped, html};
use time::Date;

use crate::layout::ViewId;
use crate::views::copy::{deadline_fr, letter_date, month_fr, year_end_fr};

fn chapter_link(href: &str, title: &str, sub: &str) -> Markup {
    html! {
        li {
            a href=(href) hx-get=(href) hx-target="#content" hx-push-url="true" {
                div {
                    strong { (title) }
                    span { (sub) }
                }
                span class="go" { "→" }
            }
        }
    }
}

fn back() -> Markup {
    html! {
        a class="back" href="/societe" hx-get="/societe" hx-target="#content" hx-push-url="true" {
            "← La société"
        }
    }
}

/// La pièce « La société » : identité courte, paysage, conversations, chapitres.
pub fn piece(store: &Store, today: Date) -> Result<Markup, AppError> {
    let home = society_home(store.connection(), today)?;
    Ok(home_markup(&home, today))
}

fn home_markup(home: &SocietyHome, today: Date) -> Markup {
    let title = home.identity.name.as_deref().unwrap_or("La société.");
    let lede = identity_lede(&home.identity);
    let pay_sub = if home.pay.possible.is_zero() {
        "Rien à sortir ce mois-ci sans casser la piste.".to_string()
    } else {
        format!(
            "{} possibles ce mois-ci sans casser la piste",
            home.pay.possible
        )
    };
    let taxes_sub = match &home.next_duty {
        Some(d) => {
            let amount = d
                .amount
                .filter(|m| *m != Money::ZERO)
                .map(|m| format!(" · {m}"))
                .unwrap_or_default();
            format!(
                "{} le {}{amount}",
                deadline_fr(d.kind),
                format_date_fr(d.due_on)
            )
        }
        None => "Aucune échéance dans l'horizon.".to_string(),
    };
    let closing_sub = match (home.closing.days_left, home.closing.unmatched) {
        (Some(days), 0) => format!("Dans {days} jours"),
        (Some(days), 1) => format!("Dans {days} jours · 1 mouvement à ranger d'abord"),
        (Some(days), n) => format!("Dans {days} jours · {n} mouvements à ranger d'abord"),
        (None, 0) => "Le parcours, en phrases.".into(),
        (None, 1) => "1 mouvement à ranger d'abord".into(),
        (None, n) => format!("{n} mouvements à ranger d'abord"),
    };
    let releve_sub = match home.unmatched {
        0 => "Tout est lu.".to_string(),
        1 => "1 mouvement sans lecture".to_string(),
        n => format!("{n} mouvements sans lecture"),
    };
    let identite_sub = match (home.identity.legal_form.as_deref(), home.identity.year_end) {
        (Some(form), Some(end)) => format!("{form} · clôture au {}", year_end_fr(end)),
        (Some(form), None) => form.to_string(),
        _ => "à renseigner".into(),
    };
    let prose = landscape_prose(&home.landscape);

    html! {
        div class="letter" data-view=(ViewId::Societe.slug()) {
            div class="date" { "La société · " (letter_date(today)) }
            h1 { (title) }
            p class="lede" { (lede) }
            (landscape_figure(&home.landscape))
            @if let Some(text) = prose {
                p class="prose" { (text) }
            }
            @if !home.conversations.bars.iter().all(|b| b.count == 0) {
                div class="block" {
                    h3 { "Les conversations, cette année" }
                    (conversation_bars(&home.conversations.bars))
                }
            }
            ul class="chapters" {
                (chapter_link("/societe/payer", "Te payer", &pay_sub))
                (chapter_link("/societe/impots", "Ce que tu dois à l'État", &taxes_sub))
                (chapter_link("/societe/cloture", "Clore l'exercice", &closing_sub))
                (chapter_link("/societe/releve", "Le relevé", &releve_sub))
                (chapter_link("/societe/identite", "L'identité", &identite_sub))
            }
        }
    }
}

fn identity_lede(id: &IdentityShort) -> String {
    match (&id.legal_form, id.capital, id.year_end, id.days_to_year_end) {
        (Some(form), capital, Some(end), Some(days)) => {
            let cap = capital
                .filter(|c| *c != Money::ZERO)
                .map(|c| format!(" au capital de {c}"))
                .unwrap_or_default();
            format!(
                "{form}{cap}. L'exercice se clôt le {}. {days} jours.",
                year_end_fr(end)
            )
        }
        (Some(form), capital, Some(end), None) => {
            let cap = capital
                .filter(|c| *c != Money::ZERO)
                .map(|c| format!(" au capital de {c}"))
                .unwrap_or_default();
            format!("{form}{cap}. L'exercice se clôt le {}.", year_end_fr(end))
        }
        (Some(form), _, _, _) => format!("{form}. La date de clôture n'est pas encore posée."),
        _ => "Dites d'abord qui vous êtes : nom, forme, clôture. Tout le reste en dépend.".into(),
    }
}

fn landscape_prose(land: &Landscape) -> Option<String> {
    if land.points.len() < 2 {
        return None;
    }
    let mut text = format!("{} aujourd'hui.", land.bank);
    if let Some(e) = &land.expected {
        text.push_str(&format!(
            " La ligne pointillée, c'est si {} verse les {}.",
            e.party, e.amount
        ));
    }
    Some(text)
}

fn landscape_figure(land: &Landscape) -> Markup {
    if land.points.len() < 2 {
        return html! {};
    }
    let svg = landscape_svg(land);
    let from = month_label(land.from.month());
    let mut right = String::from("aujourd'hui");
    if let Some(days) = land.days_to_year_end {
        right.push_str(&format!(" · clôture dans {days} jours"));
    }
    if land.expected.is_some() {
        right.push_str(" · si ça arrive");
    }
    html! {
        figure class="landscape" {
            (PreEscaped(svg))
            figcaption {
                span { (from) }
                span { (right) }
            }
        }
    }
}

fn month_label(month: u8) -> String {
    month_fr(month).chars().take(3).collect()
}

fn landscape_svg(land: &Landscape) -> String {
    let mut min = i64::MAX;
    let mut max = i64::MIN;
    for p in &land.points {
        min = min.min(p.cash.cents()).min(p.cash_if_received.cents());
        max = max.max(p.cash.cents()).max(p.cash_if_received.cents());
    }
    if min == max {
        max = min.saturating_add(1);
    }
    let n = land.points.len();
    let x_at = |i: usize| -> f64 { i as f64 / (n.saturating_sub(1).max(1) as f64) * 1200.0 };
    let y_at = |cents: i64| -> f64 {
        let t = (cents - min) as f64 / (max - min) as f64;
        148.0 - t * 120.0
    };
    let mut solid = String::new();
    for (i, p) in land.points.iter().enumerate() {
        let cmd = if i == 0 { 'M' } else { 'L' };
        solid.push_str(&format!("{cmd}{:.1},{:.1} ", x_at(i), y_at(p.cash.cents())));
    }
    let today_i = land
        .points
        .iter()
        .position(|p| p.month == land.today)
        .unwrap_or(0);
    let mut dotted = String::new();
    for (i, p) in land.points.iter().enumerate().skip(today_i) {
        let cmd = if i == today_i { 'M' } else { 'L' };
        dotted.push_str(&format!(
            "{cmd}{:.1},{:.1} ",
            x_at(i),
            y_at(p.cash_if_received.cents())
        ));
    }
    let tx = x_at(today_i);
    let ty = y_at(land.points[today_i].cash.cents());
    let wash_end = format!("L{:.1},170 L0,170 Z", x_at(n.saturating_sub(1)));
    format!(
        r##"<svg viewBox="0 0 1200 170" preserveAspectRatio="none" role="img" aria-label="paysage de trésorerie">
<defs><linearGradient id="wash" x1="0" x2="0" y1="0" y2="1"><stop offset="0" stop-color="#9f3218" stop-opacity="0.16"/><stop offset="1" stop-color="#9f3218" stop-opacity="0"/></linearGradient></defs>
<path d="{solid}{wash_end}" fill="url(#wash)"/>
<path d="{solid}" fill="none" stroke="#1c1814" stroke-width="1.5"/>
<path d="{dotted}" fill="none" stroke="#9f3218" stroke-width="1.5" stroke-dasharray="5 4"/>
<line x1="{tx:.1}" y1="12" x2="{tx:.1}" y2="158" stroke="#9f3218" stroke-width="1"/>
<circle cx="{tx:.1}" cy="{ty:.1}" r="3.5" fill="#9f3218"/>
</svg>"##
    )
}

fn conversation_bars(bars: &[ConversationBar]) -> Markup {
    let max = bars.iter().map(|b| b.count).max().unwrap_or(0).max(1);
    let cap_left = bars
        .first()
        .map(|b| month_label(b.month.month()))
        .unwrap_or_default();
    let now = bars.iter().find(|b| b.now);
    let cap_right = match now {
        Some(b) if b.thin && b.count == 1 => "aujourd'hui — un nom seulement".into(),
        Some(b) if b.thin && b.count == 2 => "aujourd'hui — deux noms seulement".into(),
        Some(b) if b.thin => format!("aujourd'hui — {} noms", b.count),
        Some(_) => "aujourd'hui".into(),
        None => String::new(),
    };
    html! {
        div class="bars" aria-hidden="true" {
            @for b in bars {
                @let h = if b.count == 0 {
                    4u32
                } else {
                    (b.count * 100 / max).max(12)
                };
                i class={ @if b.now { "now" } } style=(format!("height:{h}%")) {}
            }
        }
        div class="bars-cap" {
            span { (cap_left) }
            span { (cap_right) }
        }
    }
}

pub fn pay(store: &Store, today: Date) -> Result<Markup, AppError> {
    let pay = pay_yourself(store.connection(), today)?;
    Ok(pay_markup(&pay))
}

fn pay_markup(pay: &PayYourself) -> Markup {
    let salary_on = !matches!(pay.dividend, DividendDoor::Open { .. });
    let dividend_body = match &pay.dividend {
        DividendDoor::Closed {
            reason: DividendClosed::YearNotClosed,
        } => format!(
            "Seulement après la clôture, si l'exercice est bénéficiaire. 1 000 € pour toi coûtent {} à la société. Pas ce mois-ci : l'exercice n'est pas clos.",
            pay.dividend_cost_per_thousand
        ),
        DividendDoor::Closed {
            reason: DividendClosed::NoProfit,
        } => format!(
            "Seulement après la clôture, si l'exercice est bénéficiaire. Cette année tu as plus dépensé qu'encaissé. 1 000 € pour toi coûteraient {} à la société.",
            pay.dividend_cost_per_thousand
        ),
        DividendDoor::Open { available } => format!(
            "L'exercice est clos. {available} restent à verser. 1 000 € pour toi coûtent {} à la société.",
            pay.dividend_cost_per_thousand
        ),
    };
    let annual = match pay.annual.target {
        Some(target) if !target.is_zero() => {
            let pct = ((i128::from(pay.annual.paid.cents()) * 100)
                / i128::from(target.cents()).max(1))
            .clamp(0, 100);
            Some((pay.annual.paid, target, pct))
        }
        _ => None,
    };
    html! {
        div class="letter" data-view=(ViewId::Societe.slug()) {
            (back())
            h1 { "Te payer." }
            p class="lede" { "La question n'est pas « salaire ou dividendes ». C'est : de l'argent sur ton compte, sans casser la société." }
            div class="block" {
                h3 { "Ce mois-ci" }
                p {
                    @if pay.possible.is_zero() {
                        "Tu ne peux rien sortir ce mois-ci sans casser la piste."
                    } @else {
                        "Tu peux te verser jusqu'à " strong { (pay.possible) }
                        " et garder " (pay.runway_kept_months)
                        " mois de piste. Au-delà, tu manges le délai qui te reste pour trouver la conversation suivante."
                    }
                }
            }
            div class="pay-choice" {
                div class={ "opt" @if salary_on { " on" } } {
                    span class="mark" {}
                    div {
                        strong { "Comme un salaire" }
                        p {
                            "Chaque mois. Une fiche de paie chez ton expert (FreeFlow n'est pas un logiciel de paie). "
                            @if pay.possible.is_zero() {
                                "Rien à verser ce mois-ci sans casser la piste."
                            } @else {
                                "Toi tu reçois " (pay.salary.net) ". La société débourse "
                                (pay.salary.company_cost) ", charges comprises. Une déclaration sociale suit, en fin de mois."
                            }
                        }
                    }
                }
                div class={ "opt" @if !salary_on { " on" } @else { " shut" } } {
                    span class="mark" {}
                    div {
                        strong { "Plus tard, en dividendes" }
                        p { (dividend_body) }
                    }
                }
            }
            @if let Some((paid, target, pct)) = annual {
                div class="block" {
                    h3 { "Cette année, pour vivre" }
                    p { "Objectif " (target) ". Versé " (paid) "." }
                    div class="meter" aria-label=(format!("{paid} sur {target}")) {
                        i style=(format!("width:{pct}%")) {}
                    }
                    div class="bars-cap" {
                        span { (paid) }
                        span { (target) }
                    }
                }
            }
            div class="row-actions" {
                a class="seal" href="/societe/releve"
                  hx-get="/societe/releve" hx-target="#content" hx-push-url="true" {
                    "Le prochain virement, depuis le relevé"
                }
                a class="quiet" href="/societe"
                  hx-get="/societe" hx-target="#content" hx-push-url="true" { "Revenir" }
            }
        }
    }
}

pub fn duties(store: &Store, today: Date) -> Result<Markup, AppError> {
    let duties = society_duties(store.connection(), today)?;
    Ok(duties_markup(&duties))
}

fn duties_markup(duties: &[Duty]) -> Markup {
    html! {
        div class="letter" data-view=(ViewId::Societe.slug()) {
            (back())
            h1 { "Ce que tu dois." }
            p class="lede" { "Pas des sigles. Des dates, des montants, et où le déposer. FreeFlow prépare. Il ne transmet rien." }
            @if duties.is_empty() {
                p class="prose" { "Aucune échéance dans l'horizon." }
            } @else {
                ul class="chapters" {
                    @for d in duties {
                        li {
                            div {
                                strong { (format_date_fr(d.due_on)) " — " (deadline_fr(d.kind)) }
                                span { (duty_body(d)) }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn duty_body(d: &Duty) -> String {
    let amount = d
        .amount
        .filter(|m| *m != Money::ZERO)
        .map(|m| format!("{m} "))
        .unwrap_or_default();
    match d.deposit {
        DepositPlace::ImpotsGouv => format!(
            "{amount}à déposer sur impots.gouv.fr. Les chiffres sont déjà dans le coffre. Même à zéro, on dépose."
        ),
        DepositPlace::Post => {
            "Si tu y es assujetti. L'avis arrive par la poste, pas ici.".into()
        }
        DepositPlace::Greffe => {
            "Dépôt des comptes au greffe, via le guichet unique. FreeFlow prépare, il ne transmet pas.".into()
        }
        DepositPlace::NetEntreprises => {
            "Déclaration sociale, chez ton expert-paie. FreeFlow n'est pas un logiciel de paie.".into()
        }
        DepositPlace::Internal => {
            "Chez toi : le registre des décisions, le PV. Pas un site d'État.".into()
        }
    }
}

pub fn closing(store: &Store, today: Date) -> Result<Markup, AppError> {
    let story = closing_story(store.connection(), today)?;
    Ok(closing_markup(&story, today))
}

fn closing_markup(story: &ClosingStory, today: Date) -> Markup {
    let lede = match story.days_left {
        Some(days) if days > 0 => format!(
            "{} jours. On ne clôt pas aujourd'hui. On range, on attend, on décide.",
            french_days(days)
        ),
        Some(_) => "L'exercice est écoulé. On peut arrêter les comptes.".into(),
        None => "Le parcours, en phrases.".into(),
    };
    html! {
        div class="letter" data-view=(ViewId::Societe.slug()) {
            (back())
            h1 { "Clore." }
            p class="lede" { (lede) }
            ul class="hist" {
                @for beat in &story.beats {
                    li {
                        span class="when"
                             style=(if beat.when == BeatWhen::Today { "color:var(--seal)" } else { "" }) {
                            (beat_when_label(beat, today))
                        }
                        span { (beat_text(beat)) }
                    }
                }
            }
            @if story.unmatched > 0 {
                div class="row-actions" style="margin-top:24px" {
                    a class="seal" href="/societe/releve"
                      hx-get="/societe/releve" hx-target="#content" hx-push-url="true" {
                        "Ranger le relevé"
                    }
                }
            }
        }
    }
}

fn french_days(n: i64) -> String {
    match n {
        1 => "Un".into(),
        2 => "Deux".into(),
        n => n.to_string(),
    }
}

fn beat_when_label(beat: &ClosingBeat, today: Date) -> String {
    match beat.when {
        BeatWhen::Done => "fait".into(),
        BeatWhen::Today => "aujourd'hui".into(),
        BeatWhen::Later => beat.on.map_or_else(
            || "ensuite".into(),
            |d| {
                let _ = today;
                format!("{} {}.", d.day(), month_label(u8::from(d.month())))
            },
        ),
    }
}

fn beat_text(beat: &ClosingBeat) -> String {
    match &beat.kind {
        BeatKind::OpeningBalance { recorded: true } => "Bilan d'ouverture repris du cabinet".into(),
        BeatKind::OpeningBalance { recorded: false } => "Reprendre le bilan d'ouverture".into(),
        BeatKind::FileStatement { unmatched: 0 } => "Relevé lu".into(),
        BeatKind::FileStatement { unmatched: 1 } => "Ranger un mouvement du relevé".into(),
        BeatKind::FileStatement { unmatched } => {
            format!("Ranger {unmatched} mouvements du relevé")
        }
        BeatKind::Receipts { missing: true } => "Justificatifs manquants".into(),
        BeatKind::Receipts { missing: false } => "Justificatifs de l'exercice".into(),
        BeatKind::CloseAccounts { .. } => "Arrêter les comptes — pas avant".into(),
        BeatKind::ThenApproveAndFile => "Décider de l'affectation, faire le PV, déposer".into(),
    }
}

pub fn statement(store: &Store, today: Date) -> Result<Markup, AppError> {
    let moves = statement_moves(store.connection(), today)?;
    Ok(statement_markup(&moves))
}

fn statement_markup(moves: &[StatementMove]) -> Markup {
    html! {
        div class="letter" data-view=(ViewId::Societe.slug())
            hx-get="/societe/releve" hx-trigger="freeflow:saved from:body" hx-swap="outerHTML" {
            (back())
            h1 { "Le relevé." }
            p class="lede" { "Chaque mouvement est une phrase. Tu lui donnes une lecture. Rien n'est une « écriture »." }
            @if moves.is_empty() {
                p class="prose" { "Tout est lu." }
            }
            @for m in moves {
                article class="block" {
                    h3 { (format_date_fr(m.occurred_on)) " · " (m.amount) }
                    p { (move_phrase(m)) }
                    div class="row-actions" { (reading_actions(m)) }
                }
            }
        }
    }
}

fn move_phrase(m: &StatementMove) -> String {
    match &m.suggested {
        StatementReading::Debt { .. } => format!(
            "{}. C'est exactement la dette reprise au bilan. Ce n'est pas une charge de cette année.",
            m.description
        ),
        _ if m.description.is_empty() => "Un mouvement sans libellé.".into(),
        _ => format!("{}.", m.description),
    }
}

fn reading_actions(m: &StatementMove) -> Markup {
    let settle = format!("/depenses/transaction/{}/settle", m.id);
    let expense = format!("/depenses/new?transaction={}", m.id);
    let debt_on = matches!(m.suggested, StatementReading::Debt { .. });
    let account = match &m.suggested {
        StatementReading::Debt { account, .. } => account.as_str(),
        _ => "401000",
    };
    html! {
        @if debt_on {
            form hx-post=(settle) hx-target="#panel" hx-swap="innerHTML" {
                input type="hidden" name="account" value=(account);
                button class="seal" type="submit" { "C'est le règlement d'une dette" }
            }
        } @else {
            a class="quiet" href=(settle) hx-get=(settle) hx-target="#panel" hx-swap="innerHTML" {
                "C'est le règlement d'une dette"
            }
        }
        a class={ @if !debt_on { "seal" } @else { "quiet" } }
          href=(expense) hx-get=(expense) hx-target="#panel" hx-swap="innerHTML" {
            "C'est une dépense"
        }
        form hx-post=(settle) hx-target="#panel" hx-swap="innerHTML" {
            input type="hidden" name="account" value="455000";
            button class="quiet" type="submit" { "C'est moi que je me paie" }
        }
    }
}

pub fn identity(store: &Store, today: Date) -> Result<Markup, AppError> {
    let _ = today;
    let card = society_identity(store.connection())?;
    Ok(identity_markup(&card))
}

fn identity_markup(card: &IdentityCard) -> Markup {
    let siege = match (
        card.street.as_deref(),
        card.postal_code.as_deref(),
        card.city.as_deref(),
    ) {
        (Some(s), Some(cp), Some(c)) => Some(format!("{s}, {cp} {c}")),
        (Some(s), _, Some(c)) => Some(format!("{s}, {c}")),
        (_, _, Some(c)) => Some(c.to_string()),
        _ => None,
    };
    let president = match (
        card.president_name.as_deref(),
        card.sole_shareholder_name.as_deref(),
    ) {
        (Some(p), Some(a)) if p == a => format!("{p}, associé unique"),
        (Some(p), _) => p.to_string(),
        _ => String::new(),
    };
    html! {
        div class="letter" data-view=(ViewId::Societe.slug()) {
            (back())
            h1 { "L'identité." }
            p class="lede" { "La carte de la société, pas un formulaire de paramètres." }
            ul class="people" {
                (id_row("Nom", card.name.as_deref().unwrap_or("—")))
                (id_row(
                    "Forme",
                    &match (card.legal_form.as_deref(), card.capital) {
                        (Some(f), Some(c)) if c != Money::ZERO => format!("{f} au capital de {c}"),
                        (Some(f), _) => f.to_string(),
                        _ => "—".into(),
                    },
                ))
                @if let Some(s) = &siege {
                    (id_row("Siège", s))
                }
                @if !president.is_empty() {
                    (id_row("Président", &president))
                }
                @if let Some(s) = &card.siren {
                    (id_row("SIREN", s))
                }
                @if let Some(end) = card.year_end {
                    (id_row("Clôture", &year_end_fr(end)))
                }
                @if let Some(r) = card.vat_regime {
                    (id_row("TVA", vat_regime_fr(r)))
                }
            }
            div class="block" {
                h3 { "Le coffre" }
                p { "Un fichier sur cette machine, chiffré. Aucune connexion sortante. Les mails, la TVA, le greffe se font ailleurs — FreeFlow prépare, il ne transmet pas." }
            }
            div class="row-actions" {
                a class="quiet" href="/view/societe"
                  hx-get="/view/societe" hx-target="#content" hx-push-url="true" {
                    "Modifier la carte"
                }
            }
        }
    }
}

fn id_row(k: &str, v: &str) -> Markup {
    html! {
        li {
            div class="id-line" {
                span class="k" { (k) }
                span class="val" { (v) }
            }
        }
    }
}

fn vat_regime_fr(r: VatRegime) -> &'static str {
    match r {
        VatRegime::RealNormalMonthly => "Au réel, chaque mois",
        VatRegime::RealNormalQuarterly => "Au réel, chaque trimestre",
        VatRegime::RealSimplified => "Au réel simplifié",
        VatRegime::Franchise => "Franchise en base",
    }
}
