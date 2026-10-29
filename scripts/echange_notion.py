#!/usr/bin/env python3
"""Prépare un fichier d'échange depuis Notion, sans ouvrir le coffre.

Le script interroge les bases une fois, puis les corps de page, et écrit
``echange.json``. Il ne lance pas Griffe. La personne lit le compte-rendu,
puis tient le coffre :

    griffe echange verifier echange.json
    griffe echange appliquer echange.json

Le report, ensuite, pose ``freeflow_id``, ``Griffe`` = Importé, et la date
si elle était vide. S'il échoue, il affiche les UUID et s'arrête : il
n'applique pas une seconde fois.

``griffe echange ecrire --series phrases.json`` fournit le miroir de la
page Phrases. Le script ne la réécrit que si l'empreinte a changé.

Le jeton d'intégration se lit dans un fichier de mode 0600
(``$XDG_CONFIG_HOME/griffe/notion-jeton``, sinon
``~/.config/griffe/notion-jeton``). Il n'est jamais un argument, et il
n'entre ni dans le fichier ni dans le journal d'audit.

L'exemple est la Mairie de Valvert, conversation Refonte du site.
"""

from __future__ import annotations

import argparse
import json
import os
import stat
import sys
import urllib.error
import urllib.request
from decimal import Decimal, InvalidOperation, ROUND_CEILING, ROUND_FLOOR
from pathlib import Path

VERSION_NOTION = "2025-09-03"
VERSION_ECHANGE = 1
# Pages et select : skill griffe-import-prospects.
OPPORTUNITES = "93a1d787-40e5-4ea9-a6fe-4eb3e5132706"
ORGANISATIONS = "61995cfa-952f-4006-9248-b35a456779a3"
CONTACTS = "414faeaf-4028-4d9a-935d-4fea1052cfed"
LIGNES = "f6f78908-a88b-4642-a6e8-1c28fa7625f7"
SERIES = "a973606b-0097-401b-bdf1-94b417ea04fc"
PHRASES = "3fb47a5c-6016-8178-849a-dd7b42c8be4e"
SELECT_FILE = "À importer"
STATUTS_CLOS = frozenset({"Ignorée", "Gagnée", "Perdue"})
# Le plus long d'abord : une balise courte ne mange pas une balise longue.
VERS_COFFRE = (
    ("<signature>", "{{signature}}"),
    ("<constat>", "{{constat}}"),
    ("<montant>", "{{montant}}"),
    ("<société>", "{{societe}}"),
    ("<contact>", "{{contact}}"),
    ("<prénom>", "{{prenom}}"),
    ("<client>", "{{client}}"),
    ("<genre>", "{{civility}}"),
    ("<sujet>", "{{sujet}}"),
    ("<nom>", "{{family_name}}"),
    ("<moi>", "{{moi}}"),
)
VERS_NOTION = tuple(sorted(
    ((coffre, notion) for notion, coffre in VERS_COFFRE),
    key=lambda paire: len(paire[0]),
    reverse=True,
))


class Echec(Exception):
    """Arrêt du script. Le message ne contient pas le jeton."""


class NotionErreur(Echec):
    def __init__(self, statut: int, message: str) -> None:
        super().__init__(message)
        self.statut = statut


class ReportRate(Echec):
    """Le report Notion a échoué. Les UUID sont ceux déjà tenus dans Griffe."""

    def __init__(self, message: str, *, fiche: str | None, piste: str | None, origine: str) -> None:
        super().__init__(message)
        self.fiche = fiche
        self.piste = piste
        self.origine = origine


def chemin_jeton() -> Path:
    base = os.environ.get("XDG_CONFIG_HOME") or str(Path.home() / ".config")
    return Path(base) / "griffe" / "notion-jeton"


def lire_jeton(chemin: Path) -> str:
    """Lit le jeton. Refuse tout fichier qui n'est pas exactement 0600."""
    texte = str(chemin)
    if texte.startswith("secret_") or texte.startswith("ntn_") or "/secret_" in texte:
        raise Echec("le jeton se lit dans un fichier, pas dans la commande")
    try:
        info = os.lstat(chemin)
    except FileNotFoundError as erreur:
        raise Echec("jeton introuvable") from erreur
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode):
        raise Echec("le jeton doit être un fichier de mode 0600")
    if stat.S_IMODE(info.st_mode) != 0o600:
        raise Echec("le jeton doit être un fichier de mode 0600")
    brut = chemin.read_text(encoding="utf-8").strip()
    if not brut or any(caractere.isspace() for caractere in brut):
        raise Echec("jeton illisible")
    return brut


def rediger(texte: str, jeton: str | None) -> str:
    if jeton and jeton in texte:
        return texte.replace(jeton, "…")
    return texte


class ClientHttp:
    """API Notion. L'en-tête Authorization ne sort pas dans les erreurs."""

    def __init__(self, jeton: str) -> None:
        self.jeton = jeton

    def appel(self, methode: str, chemin: str, corps: dict | None) -> dict:
        donnees = None if corps is None else json.dumps(corps).encode("utf-8")
        requete = urllib.request.Request(
            "https://api.notion.com" + chemin,
            data=donnees,
            method=methode,
            headers={
                "Authorization": "Bearer " + self.jeton,
                "Notion-Version": VERSION_NOTION,
                "Accept": "application/json",
            },
        )
        if donnees is not None:
            requete.add_header("Content-Type", "application/json")
        try:
            with urllib.request.urlopen(requete, timeout=60) as reponse:
                brut = reponse.read().decode("utf-8")
        except urllib.error.HTTPError as erreur:
            raise NotionErreur(erreur.code, self._message(erreur)) from None
        if not brut:
            return {}
        return json.loads(brut)

    def _message(self, erreur: urllib.error.HTTPError) -> str:
        try:
            charge = json.loads(erreur.read().decode("utf-8"))
        except (json.JSONDecodeError, UnicodeError, OSError):
            charge = {}
        message = ""
        if isinstance(charge, dict):
            message = str(charge.get("message") or "")
        return rediger(message or f"notion {erreur.code}", self.jeton)


def normaliser_id(valeur: str) -> str:
    brut = (valeur or "").replace("-", "").strip().lower()
    if len(brut) == 32 and all(caractere in "0123456789abcdef" for caractere in brut):
        return f"{brut[0:8]}-{brut[8:12]}-{brut[12:16]}-{brut[16:20]}-{brut[20:]}"
    return (valeur or "").strip()


def interroger(transport, source: str, corps: dict) -> list:
    """Une base, suivie de son curseur. Pas une requête par dossier."""
    resultats: list = []
    curseur = None
    while True:
        envoi = dict(corps)
        envoi["page_size"] = 100
        if curseur:
            envoi["start_cursor"] = curseur
        page = transport.appel("POST", f"/v1/data_sources/{source}/query", envoi)
        for ligne in page.get("results") or []:
            if ligne.get("object") in (None, "page"):
                resultats.append(ligne)
        if not page.get("has_more"):
            return resultats
        curseur = page.get("next_cursor")
        if not curseur:
            return resultats


def enfants(transport, page_id: str) -> list:
    blocs: list = []
    curseur = None
    while True:
        chemin = f"/v1/blocks/{normaliser_id(page_id)}/children?page_size=100"
        if curseur:
            chemin += f"&start_cursor={urllib.request.quote(str(curseur))}"
        page = transport.appel("GET", chemin, None)
        blocs.extend(page.get("results") or [])
        if not page.get("has_more"):
            return blocs
        curseur = page.get("next_cursor")
        if not curseur:
            return blocs


def vivant(page: dict) -> bool:
    return not page.get("archived") and not page.get("in_trash")


def indexer(pages: list) -> dict:
    index = {}
    for page in pages:
        if not vivant(page):
            continue
        identifiant = normaliser_id(str(page.get("id") or ""))
        if identifiant:
            index[identifiant] = page
    return index


def propriete(page: dict, nom: str) -> dict | None:
    props = page.get("properties") or {}
    valeur = props.get(nom)
    return valeur if isinstance(valeur, dict) else None


def _plain(morceaux) -> str:
    textes = []
    for morceau in morceaux or []:
        if not isinstance(morceau, dict):
            continue
        # Notion renvoie plain_text. Un bloc que le script vient d'écrire
        # n'a que text.content : l'empreinte Phrases compare les deux.
        plain = morceau.get("plain_text")
        if plain:
            textes.append(str(plain))
            continue
        texte = morceau.get("text")
        if isinstance(texte, dict):
            textes.append(str(texte.get("content") or ""))
    return "".join(textes)


def texte(prop: dict | None):
    if not prop:
        return ""
    sorte = prop.get("type")
    if sorte == "title":
        return _plain(prop.get("title"))
    if sorte == "rich_text":
        return _plain(prop.get("rich_text"))
    if sorte == "select":
        choix = prop.get("select") or {}
        return str(choix.get("name") or "")
    if sorte == "status":
        choix = prop.get("status") or {}
        return str(choix.get("name") or "")
    if sorte in {"email", "url", "phone_number"}:
        return str(prop.get(sorte) or "")
    if sorte == "number":
        return prop.get("number")
    if sorte == "date":
        date = prop.get("date") or {}
        return str(date.get("start") or "")
    return ""


def texte_nomme(page: dict, nom: str) -> str:
    valeur = texte(propriete(page, nom))
    return valeur.strip() if isinstance(valeur, str) else ""


def titre_page(page: dict) -> str:
    for prop in (page.get("properties") or {}).values():
        if isinstance(prop, dict) and prop.get("type") == "title":
            return _plain(prop.get("title")).strip()
    return ""


def qui_de(page: dict) -> str:
    nomme = texte_nomme(page, "Qui")
    return nomme or titre_page(page)


def relations(prop: dict | None) -> list[str]:
    if not prop or prop.get("type") != "relation":
        return []
    identifiants = []
    for item in prop.get("relation") or []:
        identifiant = normaliser_id(str(item.get("id") or ""))
        if identifiant:
            identifiants.append(identifiant)
    return identifiants


def relations_completes(transport, page: dict, nom: str) -> list[str]:
    prop = propriete(page, nom)
    identifiants = relations(prop)
    if not prop or not prop.get("has_more"):
        return identifiants
    prop_id = prop.get("id")
    if not prop_id:
        return identifiants
    page_id = normaliser_id(str(page.get("id") or ""))
    trouves: list[str] = []
    curseur = None
    while True:
        chemin = f"/v1/pages/{page_id}/properties/{prop_id}?page_size=100"
        if curseur:
            chemin += f"&start_cursor={urllib.request.quote(str(curseur))}"
        page_prop = transport.appel("GET", chemin, None)
        for item in page_prop.get("results") or []:
            relation = item.get("relation") or {}
            identifiant = normaliser_id(str(relation.get("id") or ""))
            if identifiant:
                trouves.append(identifiant)
        if not page_prop.get("has_more"):
            return trouves
        curseur = page_prop.get("next_cursor")
        if not curseur:
            return trouves


def completer_relations(transport, pages: list) -> None:
    """Une relation est tronquée à la première page. On la relit si ``has_more``."""
    for page in pages:
        if not isinstance(page, dict) or not vivant(page):
            continue
        for nom, prop in (page.get("properties") or {}).items():
            if not isinstance(prop, dict) or prop.get("type") != "relation":
                continue
            if not prop.get("has_more"):
                continue
            identifiants = relations_completes(transport, page, nom)
            prop["relation"] = [{"id": identifiant} for identifiant in identifiants]
            prop["has_more"] = False


def type_propriete(page: dict | None, nom: str, defaut: str) -> str:
    if not page:
        return defaut
    prop = propriete(page, nom)
    if not prop or not prop.get("type"):
        return defaut
    return str(prop["type"])


def centimes_proches(valeur) -> str | None:
    """``1200,00`` si la valeur tombe sur un centime, à un millième près."""
    if isinstance(valeur, bool) or valeur is None:
        return None
    try:
        if isinstance(valeur, str):
            montant = Decimal(
                valeur.strip().replace(" ", "").replace("\u00a0", "").replace(",", ".")
            )
        elif isinstance(valeur, int):
            montant = Decimal(valeur)
        elif isinstance(valeur, float):
            montant = Decimal(str(valeur))
        else:
            return None
    except InvalidOperation:
        return None
    centimes = montant * Decimal(100)
    bas = centimes.to_integral_value(rounding=ROUND_FLOOR)
    haut = centimes.to_integral_value(rounding=ROUND_CEILING)
    entier = bas if abs(centimes - bas) <= abs(centimes - haut) else haut
    if abs(centimes - entier) > Decimal("0.001") or entier <= 0:
        return None
    total = int(entier)
    return f"{total // 100},{total % 100:02d}"


def montant_ligne(valeur) -> str:
    exact = centimes_proches(valeur)
    if exact is not None:
        return exact
    if isinstance(valeur, bool) or valeur is None:
        return "0,00"
    if isinstance(valeur, str):
        return valeur.strip()
    if isinstance(valeur, (int, float)):
        texte_nombre = format(Decimal(str(valeur)), "f")
        if "." not in texte_nombre:
            texte_nombre += ".00"
        return texte_nombre.replace(".", ",")
    return str(valeur).strip()


def jetons_coffre(texte: str) -> str:
    for source, cible in VERS_COFFRE:
        texte = texte.replace(source, cible)
    return texte


def jetons_notion(texte: str) -> str:
    for source, cible in VERS_NOTION:
        texte = texte.replace(source, cible)
    return texte


def texte_bloc(bloc: dict) -> str:
    sorte = bloc.get("type")
    charge = bloc.get(sorte) or {}
    if not isinstance(charge, dict):
        return ""
    if "rich_text" in charge:
        return _plain(charge.get("rich_text"))
    if sorte == "child_page":
        return str(charge.get("title") or "")
    return ""


def sections_h2(blocs: list) -> dict[str, list]:
    sections: dict[str, list] = {}
    courant: str | None = None
    tampon: list = []

    def ranger() -> None:
        if courant is not None:
            sections[courant] = list(tampon)

    for bloc in blocs:
        if bloc.get("type") == "heading_2":
            ranger()
            courant = texte_bloc(bloc).strip()
            tampon = []
            continue
        if courant is None:
            continue
        if bloc.get("type") in {"child_page", "child_database", "link_to_page"}:
            continue
        tampon.append(bloc)
    ranger()
    return sections


def markdown_blocs(blocs: list, *, brouillon: bool) -> str:
    lignes: list[str] = []
    for bloc in blocs:
        sorte = bloc.get("type")
        brut = texte_bloc(bloc).strip()
        if sorte in {"heading_1", "heading_2"}:
            continue
        if brouillon and sorte == "heading_3" and brut in {"Sujet", "Corps"}:
            continue
        if not brouillon and sorte == "heading_3":
            if brut:
                lignes.append(f"### {brut}")
            continue
        if sorte == "bulleted_list_item" and brut:
            lignes.append(f"- {brut}")
            continue
        if sorte == "numbered_list_item" and brut:
            lignes.append(f"1. {brut}")
            continue
        if brut:
            lignes.append(brut)
    return "\n\n".join(lignes).strip()


def moments_de(blocs: list) -> list[dict]:
    moments: list[dict] = []
    cle: str | None = None
    zone: str | None = None
    sujet: list[str] = []
    corps: list[str] = []

    def ranger() -> None:
        if not cle:
            return
        moments.append({
            "cle": cle,
            "sujet": jetons_coffre("\n\n".join(sujet).strip()),
            "corps": jetons_coffre("\n\n".join(corps).strip()),
        })

    for bloc in blocs:
        sorte = bloc.get("type")
        brut = texte_bloc(bloc).strip()
        if sorte == "heading_2":
            ranger()
            cle = brut
            zone = None
            sujet = []
            corps = []
            continue
        if cle is None:
            continue
        if sorte == "heading_3" and brut == "Sujet":
            zone = "sujet"
            continue
        if sorte == "heading_3" and brut == "Corps":
            zone = "corps"
            continue
        if not brut or zone is None:
            continue
        (sujet if zone == "sujet" else corps).append(brut)
    ranger()
    return moments


def date_fichier(brut: str, aujourdhui: str) -> tuple[str, bool]:
    brut = (brut or "").strip()
    if not brut:
        return aujourdhui, True
    if len(brut) >= 10 and brut[4] == "-" and brut[7] == "-" and (len(brut) == 10 or brut[10] in "T "):
        jour = brut[:10]
        if len(jour) == 10 and jour[4] == "-" and jour[7] == "-":
            return jour, False
    return brut, False


def contact_de(page: dict) -> dict | None:
    prenom = texte_nomme(page, "Prénom")
    nom = texte_nomme(page, "Nom")
    if not prenom or not nom:
        return None
    contact = {"prenom": prenom, "nom": nom}
    genre = texte_nomme(page, "Genre")
    if genre == "Monsieur":
        contact["genre"] = "monsieur"
    elif genre == "Madame":
        contact["genre"] = "madame"
    courriel = texte_nomme(page, "Email") or texte_nomme(page, "Courriel")
    telephone = texte_nomme(page, "Téléphone")
    if courriel:
        contact["courriel"] = courriel
    if telephone:
        contact["telephone"] = telephone
    return contact


def adresse_de(page: dict) -> dict | None:
    rue = texte_nomme(page, "Rue")
    code = texte_nomme(page, "Code postal")
    ville = texte_nomme(page, "Ville")
    if not rue or not code or not ville:
        return None
    return {"rue": rue, "code_postal": code, "ville": ville, "pays": "FR"}


def ligne_de(page: dict) -> dict:
    mission = texte_nomme(page, "Mission") or titre_page(page)
    montant = texte(propriete(page, "Montant HT"))
    return {"libelle": mission, "montant": montant_ligne(montant)}


def serie_par_nom(pages: list) -> dict[str, dict]:
    index = {}
    for page in pages:
        if not vivant(page):
            continue
        nom = titre_page(page)
        if nom and nom not in index:
            index[nom] = page
    return index


def poser(dossier: dict, cle: str, valeur) -> None:
    if valeur is None:
        return
    if isinstance(valeur, str) and not valeur.strip():
        return
    dossier[cle] = valeur


def assembler(file_pages, organisations, contacts, lignes, series, blocs, aujourdhui: str) -> tuple[dict, dict]:
    """Construit le document. ``reprendre`` reste absent : un report ou un brouillon tenu dans Griffe reste."""
    orgs = indexer(organisations)
    gens = indexer(contacts)
    missions = indexer(lignes)
    proposes = serie_par_nom(series)
    dossiers = []
    suivi_dossiers = []
    incomplets = []
    series_vues: list[str] = []
    quoi_repris = []

    for page in file_pages:
        if not vivant(page):
            continue
        origine = normaliser_id(str(page.get("id") or ""))
        orgs_ids = relations(propriete(page, "Organisation"))
        organisation = orgs.get(orgs_ids[0]) if orgs_ids else None
        qui = qui_de(organisation) if organisation else ""
        statut = texte_nomme(page, "Statut")
        if statut in STATUTS_CLOS:
            incomplets.append({"origine": origine, "qui": qui, "raison": "statut clos"})
            continue
        if organisation is None:
            incomplets.append({"origine": origine, "qui": qui, "raison": "sans organisation"})
            continue
        corps = blocs.get(origine, [])
        sections = sections_h2(corps)
        if "Travaux" not in sections:
            incomplets.append({"origine": origine, "qui": qui, "raison": "sans travaux"})
            continue
        quoi = texte_nomme(page, "Quoi")
        if not quoi:
            quoi = titre_page(page)
            quoi_repris.append(origine)
        date, date_vide = date_fichier(texte_nomme(page, "Date prochaine action"), aujourdhui)
        dossier: dict = {
            "origine": origine,
            "origine_fiche": normaliser_id(str(organisation.get("id") or orgs_ids[0])),
            "qui": qui,
            "quoi": quoi,
        }
        poser(dossier, "adresse", adresse_de(organisation))
        poser(dossier, "courriel", texte_nomme(organisation, "Courriel"))
        poser(dossier, "telephone", texte_nomme(organisation, "Téléphone"))
        travaux = markdown_blocs(sections.get("Travaux") or [], brouillon=False)
        poser(dossier, "travaux", travaux or None)
        contact = None
        for identifiant in relations(propriete(page, "Contact")):
            page_contact = gens.get(identifiant)
            if page_contact is None:
                continue
            contact = contact_de(page_contact)
            if contact is not None:
                break
        poser(dossier, "contact", contact)
        dossier["prochaine_action"] = date
        poser(dossier, "source", texte_nomme(page, "Source"))
        lignes_doc = []
        for identifiant in relations(propriete(page, "Lignes")):
            page_ligne = missions.get(identifiant)
            if page_ligne is None:
                continue
            lignes_doc.append(ligne_de(page_ligne))
        if lignes_doc:
            dossier["lignes"] = lignes_doc
        serie = texte_nomme(page, "Série")
        poser(dossier, "serie", serie or None)
        if serie and serie not in series_vues and serie in proposes:
            series_vues.append(serie)
        brouillon = None
        if "Premier message" in sections:
            brouillon = jetons_coffre(
                markdown_blocs(sections["Premier message"], brouillon=True)
            )
        poser(dossier, "brouillon", brouillon or None)
        dossiers.append(dossier)
        suivi_dossiers.append({
            "origine": origine,
            "origine_fiche": dossier["origine_fiche"],
            "qui": qui,
            "date_vide": date_vide,
            "prochaine_action": date,
        })

    series_doc = []
    for nom in series_vues:
        page = proposes[nom]
        identifiant = normaliser_id(str(page.get("id") or ""))
        series_doc.append({"nom": nom, "moments": moments_de(blocs.get(identifiant, []))})

    document: dict = {"version": VERSION_ECHANGE, "dossiers": dossiers}
    if series_doc:
        document["series"] = series_doc
    premiere = next((page for page in file_pages if vivant(page)), None)
    premiere_org = None
    if premiere:
        ids = relations(propriete(premiere, "Organisation"))
        premiere_org = orgs.get(ids[0]) if ids else None
    suivi = {
        "version": VERSION_ECHANGE,
        "jour": aujourdhui,
        "types": {
            "freeflow_id": type_propriete(premiere, "freeflow_id", "rich_text"),
            "Griffe": type_propriete(premiere, "Griffe", "select"),
            "Date prochaine action": type_propriete(premiere, "Date prochaine action", "date"),
        },
        "types_fiche": {
            "freeflow_id": type_propriete(premiere_org, "freeflow_id", "rich_text"),
        },
        "dossiers": suivi_dossiers,
        "incomplets": incomplets,
        "quoi_repris": quoi_repris,
    }
    return document, suivi


def lire(transport, aujourdhui: str) -> tuple[dict, dict]:
    file_pages = interroger(
        transport,
        OPPORTUNITES,
        {"filter": {"property": "Griffe", "select": {"equals": SELECT_FILE}}},
    )
    if not file_pages:
        document = {"version": VERSION_ECHANGE, "dossiers": []}
        suivi = {
            "version": VERSION_ECHANGE,
            "jour": aujourdhui,
            "types": {
                "freeflow_id": "rich_text",
                "Griffe": "select",
                "Date prochaine action": "date",
            },
            "types_fiche": {"freeflow_id": "rich_text"},
            "dossiers": [],
            "incomplets": [],
            "quoi_repris": [],
        }
        return document, suivi
    organisations = interroger(transport, ORGANISATIONS, {})
    contacts = interroger(transport, CONTACTS, {})
    lignes = interroger(transport, LIGNES, {})
    series = interroger(transport, SERIES, {})
    completer_relations(transport, file_pages)
    # Les corps viennent après les cinq bases.
    a_lire = []
    orgs = indexer(organisations)
    for page in file_pages:
        if not vivant(page):
            continue
        if texte_nomme(page, "Statut") in STATUTS_CLOS:
            continue
        ids = relations(propriete(page, "Organisation"))
        if not ids or ids[0] not in orgs:
            continue
        a_lire.append(normaliser_id(str(page.get("id") or "")))
    blocs = {identifiant: enfants(transport, identifiant) for identifiant in a_lire}
    proposes = serie_par_nom(series)
    noms = []
    for page in file_pages:
        if normaliser_id(str(page.get("id") or "")) not in blocs:
            continue
        nom = texte_nomme(page, "Série")
        if nom and nom in proposes and nom not in noms:
            noms.append(nom)
    for nom in noms:
        identifiant = normaliser_id(str(proposes[nom].get("id") or ""))
        if identifiant and identifiant not in blocs:
            blocs[identifiant] = enfants(transport, identifiant)
    return assembler(file_pages, organisations, contacts, lignes, series, blocs, aujourdhui)


def ecrire_json(chemin: Path, valeur: dict) -> None:
    chemin.parent.mkdir(parents=True, exist_ok=True)
    chemin.write_text(json.dumps(valeur, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def chemin_suivi(sortie: Path) -> Path:
    return sortie.with_name(sortie.stem + ".suivi.json")


def afficher_lecture(document: dict, suivi: dict) -> None:
    if not document["dossiers"] and not suivi["incomplets"]:
        print("rien à importer")
        return
    repris = set(suivi.get("quoi_repris") or [])
    for dossier in document["dossiers"]:
        nombre = len(dossier.get("lignes") or [])
        ligne = (
            f"{dossier['qui']} — {dossier['quoi']} — {nombre} lignes — "
            f"{dossier['prochaine_action']}"
        )
        if dossier["origine"] in repris:
            ligne += " — Quoi repris du titre"
        print(ligne)
    for incomplet in suivi["incomplets"]:
        qui = incomplet.get("qui") or incomplet["origine"]
        print(f"incomplet — {qui} — {incomplet['raison']}")


def prop_texte(sorte: str, valeur: str) -> dict:
    morceau = [{"type": "text", "text": {"content": valeur}}]
    if sorte == "title":
        return {"title": morceau}
    if sorte == "email":
        return {"email": valeur}
    if sorte == "phone_number":
        return {"phone_number": valeur}
    return {"rich_text": morceau}


def prop_choix(sorte: str, valeur: str) -> dict:
    if sorte == "status":
        return {"status": {"name": valeur}}
    return {"select": {"name": valeur}}


def proprietes_importe(ligne: dict, piste: str, types: dict) -> dict:
    proprietes = {
        "freeflow_id": prop_texte(types.get("freeflow_id") or "rich_text", piste),
        "Griffe": prop_choix(types.get("Griffe") or "select", "Importé"),
    }
    if ligne.get("date_vide"):
        proprietes["Date prochaine action"] = {"date": {"start": ligne["prochaine_action"]}}
    return proprietes


def proprietes_fiche(fiche: str, types_fiche: dict) -> dict:
    return {"freeflow_id": prop_texte(types_fiche.get("freeflow_id") or "rich_text", fiche)}


def proprietes_incomplet(types: dict) -> dict:
    return {"Griffe": prop_choix(types.get("Griffe") or "select", "Incomplet")}


def _patch(transport, page_id: str, proprietes: dict, *, fiche, piste, origine: str) -> None:
    try:
        transport.appel(
            "PATCH",
            f"/v1/pages/{normaliser_id(page_id)}",
            {"properties": proprietes},
        )
    except NotionErreur as erreur:
        raise ReportRate(str(erreur), fiche=fiche, piste=piste, origine=origine) from None


def charger_rapport(chemin: Path) -> dict:
    try:
        brut = json.loads(chemin.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as erreur:
        raise Echec("compte-rendu illisible") from erreur
    if not isinstance(brut, dict):
        raise Echec("compte-rendu illisible")
    if "status" in brut:
        if brut["status"] != "applied":
            raise Echec(f"le compte-rendu n'est pas une application : {brut['status']}")
        resultat = brut.get("result")
        if not isinstance(resultat, dict):
            raise Echec("compte-rendu illisible")
        return resultat
    return brut


def reporter(transport, rapport: dict, suivi: dict) -> None:
    """Écrit Notion. N'appelle pas Griffe, même si une écriture échoue."""
    types = suivi.get("types") or {}
    types_fiche = suivi.get("types_fiche") or {}
    par_origine = {ligne["origine"]: ligne for ligne in suivi.get("dossiers") or []}
    vus = set()
    for dossier in rapport.get("dossiers") or []:
        origine = str(dossier.get("origine") or "")
        vus.add(origine)
        fiche = dossier.get("fiche")
        piste = dossier.get("piste")
        if dossier.get("statut") == "incomplet" or not fiche or not piste:
            _patch(
                transport,
                origine,
                proprietes_incomplet(types),
                fiche=None,
                piste=None,
                origine=origine,
            )
            print(f"incomplet — {dossier.get('qui') or origine}")
            continue
        fiche = str(fiche)
        piste = str(piste)
        ligne = par_origine.get(origine) or {}
        _patch(
            transport,
            origine,
            proprietes_importe(ligne, piste, types),
            fiche=fiche,
            piste=piste,
            origine=origine,
        )
        origine_fiche = ligne.get("origine_fiche")
        if origine_fiche:
            _patch(
                transport,
                origine_fiche,
                proprietes_fiche(fiche, types_fiche),
                fiche=fiche,
                piste=piste,
                origine=origine,
            )
        print(f"Importé — {dossier.get('qui') or origine} — {piste}")
    for incomplet in suivi.get("incomplets") or []:
        origine = incomplet["origine"]
        if origine in vus:
            continue
        _patch(
            transport,
            origine,
            proprietes_incomplet(types),
            fiche=None,
            piste=None,
            origine=origine,
        )
        print(f"incomplet — {incomplet.get('qui') or origine}")


def rich_text(contenu: str) -> list:
    morceaux = []
    reste = contenu
    while reste:
        morceaux.append({"type": "text", "text": {"content": reste[:2000]}})
        reste = reste[2000:]
    if not morceaux:
        morceaux.append({"type": "text", "text": {"content": ""}})
    return morceaux


def bloc_titre(niveau: int, texte: str) -> dict:
    cle = f"heading_{niveau}"
    return {"object": "block", "type": cle, cle: {"rich_text": rich_text(texte)}}


def bloc_paragraphe(texte: str) -> dict:
    return {"object": "block", "type": "paragraph", "paragraph": {"rich_text": rich_text(texte)}}


def bloc_code(texte: str) -> dict:
    return {
        "object": "block",
        "type": "code",
        "code": {"rich_text": rich_text(texte), "language": "plain text"},
    }


def rendre_moment(cle: str, sujet: str, corps: str) -> list:
    return [
        bloc_titre(3, cle),
        bloc_paragraphe("Sujet"),
        bloc_code(jetons_notion(sujet or "")),
        bloc_paragraphe("Corps"),
        bloc_code(jetons_notion(corps or "")),
    ]


def rendre_phrases(catalogue: dict) -> list:
    blocs: list = []
    cadence = catalogue.get("cadence") or []
    if cadence:
        blocs.append(bloc_titre(2, "Défaut"))
        for moment in cadence:
            blocs.extend(rendre_moment(
                str(moment.get("cle") or ""),
                str(moment.get("sujet") or ""),
                str(moment.get("corps") or ""),
            ))
    for serie in catalogue.get("series") or []:
        blocs.append(bloc_titre(2, str(serie.get("nom") or "")))
        for moment in serie.get("moments") or []:
            blocs.extend(rendre_moment(
                str(moment.get("cle") or ""),
                str(moment.get("sujet") or ""),
                str(moment.get("corps") or ""),
            ))
    return blocs


def empreinte_blocs(blocs: list) -> tuple:
    return tuple((bloc.get("type"), texte_bloc(bloc)) for bloc in blocs)


def miroiter(transport, catalogue: dict) -> str:
    if catalogue.get("version") != VERSION_ECHANGE:
        raise Echec("version d'échange inconnue")
    voulus = rendre_phrases(catalogue)
    actuels = enfants(transport, PHRASES)
    preambule = 3 if len(actuels) >= 3 else len(actuels)
    queue = actuels[preambule:]
    if empreinte_blocs(queue) == empreinte_blocs(voulus):
        return "phrases inchangées"
    for bloc in queue:
        identifiant = bloc.get("id")
        if not identifiant:
            continue
        try:
            transport.appel("DELETE", f"/v1/blocks/{identifiant}", None)
        except NotionErreur as erreur:
            raise Echec(str(erreur)) from None
    for debut in range(0, len(voulus), 100):
        try:
            transport.appel(
                "PATCH",
                f"/v1/blocks/{PHRASES}/children",
                {"children": voulus[debut:debut + 100]},
            )
        except NotionErreur as erreur:
            raise Echec(str(erreur)) from None
    return "phrases réécrites"


def ecrire_report_rate(erreur: ReportRate, jeton: str | None) -> None:
    print("Le report n'a pas abouti.", file=sys.stderr)
    if erreur.piste:
        print(f"piste {erreur.piste}", file=sys.stderr)
    if erreur.fiche:
        print(f"fiche {erreur.fiche}", file=sys.stderr)
    if erreur.origine and not erreur.piste:
        print(f"opportunité {erreur.origine}", file=sys.stderr)
    message = rediger(str(erreur), jeton)
    if message:
        print(message, file=sys.stderr)


def _jour(valeur: str | None) -> str:
    if valeur:
        if len(valeur) != 10 or valeur[4] != "-" or valeur[7] != "-":
            raise Echec("date illisible")
        return valeur
    from datetime import date
    return date.today().isoformat()


def construire_analyseur() -> argparse.ArgumentParser:
    commun = argparse.ArgumentParser(add_help=False)
    commun.add_argument("--jeton", type=Path, default=None)
    analyseur = argparse.ArgumentParser(
        description="Fichier d'échange Notion. N'ouvre pas le coffre.",
    )
    sous = analyseur.add_subparsers(dest="geste", required=True)

    lire_cmd = sous.add_parser("lire", parents=[commun], help="écrit echange.json")
    lire_cmd.add_argument("--sortie", type=Path, default=Path("echange.json"))
    lire_cmd.add_argument("--aujourdhui", default=None)

    report = sous.add_parser(
        "reporter", parents=[commun], help="pose Importé après l'application",
    )
    report.add_argument("--compte-rendu", type=Path, required=True)
    report.add_argument("--suivi", type=Path, default=Path("echange.suivi.json"))

    phrases = sous.add_parser("phrases", parents=[commun], help="miroir de la page Phrases")
    phrases.add_argument("--series", type=Path, required=True)
    return analyseur


def normaliser_argv(argv: list[str]) -> list[str]:
    if not argv or argv[0] in {"-h", "--help"}:
        return argv
    if argv[0] not in {"lire", "reporter", "phrases"}:
        return ["lire", *argv]
    return argv


def commande(argv: list[str], transport=None) -> int:
    argv = normaliser_argv(argv)
    if not argv or argv[0] in {"-h", "--help"}:
        construire_analyseur().print_help()
        return 0
    args = construire_analyseur().parse_args(argv)
    jeton = None
    try:
        jeton = lire_jeton(args.jeton or chemin_jeton())
        client = transport if transport is not None else ClientHttp(jeton)
        if args.geste == "lire":
            document, suivi = lire(client, _jour(args.aujourdhui))
            ecrire_json(args.sortie, document)
            ecrire_json(chemin_suivi(args.sortie), suivi)
            afficher_lecture(document, suivi)
            return 0
        if args.geste == "reporter":
            rapport = charger_rapport(args.compte_rendu)
            try:
                suivi = json.loads(args.suivi.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError) as erreur:
                raise Echec("suivi illisible") from erreur
            reporter(client, rapport, suivi)
            return 0
        try:
            catalogue = json.loads(args.series.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as erreur:
            raise Echec("catalogue illisible") from erreur
        print(miroiter(client, catalogue))
        return 0
    except ReportRate as erreur:
        ecrire_report_rate(erreur, jeton)
        return 1
    except Echec as erreur:
        print(rediger(str(erreur), jeton), file=sys.stderr)
        return 1


def main(argv: list[str] | None = None) -> int:
    return commande(sys.argv[1:] if argv is None else argv)


if __name__ == "__main__":
    sys.exit(main())
