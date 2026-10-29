#!/usr/bin/env python3
"""Le script Notion, sans réseau, sur la Mairie de Valvert."""

from __future__ import annotations

import contextlib
import io
import json
import os
import sys
import tempfile
import unittest
import urllib.error
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import echange_notion as notion

FIXTURES = Path(__file__).resolve().parent / "testdata" / "echange"
JETON = "jeton-secret-valvert"
PISTE = "dddddddd-dddd-4ddd-8ddd-dddddddddddd"
FICHE = "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee"
JOUR = "2026-10-28"


def lancer(argv, transport):
    sortie = io.StringIO()
    erreur = io.StringIO()
    with contextlib.redirect_stdout(sortie), contextlib.redirect_stderr(erreur):
        code = notion.commande(list(argv), transport)
    return code, sortie.getvalue(), erreur.getvalue()


def ecrire_jeton(dossier: Path, mode: int = 0o600, texte: str = JETON) -> Path:
    chemin = dossier / "jeton"
    chemin.write_text(texte + "\n", encoding="utf-8")
    os.chmod(chemin, mode)
    return chemin


class Memoire:
    """Transport Notion en mémoire. ``urlopen`` reste empoisonné."""

    def __init__(self):
        self.journal = []
        self.sources = {}
        self.enfants = {}
        self.proprietes = {}
        self.echec_pages = {}

    def appel(self, methode, chemin, corps):
        copie = json.loads(json.dumps(corps)) if corps is not None else None
        self.journal.append((methode, chemin, copie))
        nu = chemin.split("?", 1)[0]
        parts = nu.split("/")
        if methode == "POST" and nu.startswith("/v1/data_sources/") and nu.endswith("/query"):
            source = parts[3]
            if source not in self.sources:
                raise AssertionError(f"source inattendue {source}")
            return {"object": "list", "results": self.sources[source], "has_more": False}
        if methode == "GET" and nu.startswith("/v1/blocks/") and nu.endswith("/children"):
            identifiant = parts[3]
            if identifiant not in self.enfants:
                raise AssertionError(f"corps inattendu {identifiant}")
            return {"object": "list", "results": list(self.enfants[identifiant]), "has_more": False}
        if methode == "GET" and "/properties/" in nu:
            cle = (parts[3], parts[5])
            if cle not in self.proprietes:
                raise AssertionError(f"propriété inattendue {cle}")
            return {
                "results": [
                    {"type": "relation", "relation": {"id": identifiant}}
                    for identifiant in self.proprietes[cle]
                ],
                "has_more": False,
            }
        if methode == "PATCH" and nu.startswith("/v1/pages/"):
            page_id = parts[3]
            if page_id in self.echec_pages:
                raise notion.NotionErreur(400, self.echec_pages[page_id])
            return {}
        if methode == "DELETE" and nu.startswith("/v1/blocks/"):
            bloc_id = parts[3]
            for identifiant, blocs in list(self.enfants.items()):
                self.enfants[identifiant] = [bloc for bloc in blocs if bloc.get("id") != bloc_id]
            return {}
        if methode == "PATCH" and nu.startswith("/v1/blocks/") and nu.endswith("/children"):
            self.enfants.setdefault(parts[3], []).extend((corps or {}).get("children") or [])
            return {}
        raise AssertionError(f"appel inattendu {methode} {chemin}")

    def charger_valvert(self):
        brut = json.loads((FIXTURES / "valvert-reponses.json").read_text(encoding="utf-8"))
        self.sources = brut["sources"]
        self.enfants = brut["enfants"]


def titre(texte):
    return {"type": "title", "title": [{"plain_text": texte}]}


def riche(texte):
    return {"type": "rich_text", "rich_text": [{"plain_text": texte}] if texte else []}


def page(identifiant, proprietes, **extra):
    base = {"object": "page", "id": identifiant, "archived": False, "in_trash": False, "properties": proprietes}
    base.update(extra)
    return base


def paragraphe(texte):
    return {"type": "paragraph", "paragraph": {"rich_text": [{"plain_text": texte}]}}


def titre2(texte):
    return {"type": "heading_2", "heading_2": {"rich_text": [{"plain_text": texte}]}}


class SansReseau(unittest.TestCase):
    def setUp(self):
        self._urlopen = urllib.request.urlopen

        def interdit(*_args, **_kwargs):
            raise AssertionError("appel réseau")

        urllib.request.urlopen = interdit

    def tearDown(self):
        urllib.request.urlopen = self._urlopen

    def test_valvert_ecrit_le_document_sans_ecrire_notion(self):
        attendu = json.loads((FIXTURES / "valvert.json").read_text(encoding="utf-8"))
        with tempfile.TemporaryDirectory() as dossier:
            dossier = Path(dossier)
            jeton = ecrire_jeton(dossier)
            sortie = dossier / "echange.json"
            transport = Memoire()
            transport.charger_valvert()
            code, out, err = lancer(
                ["lire", "--jeton", str(jeton), "--sortie", str(sortie), "--aujourdhui", JOUR],
                transport,
            )
            self.assertEqual(code, 0, err)
            self.assertEqual(err, "")
            document = json.loads(sortie.read_text(encoding="utf-8"))
            self.assertEqual(document, attendu)
            self.assertNotIn(JETON, sortie.read_text(encoding="utf-8"))
            self.assertNotIn(JETON, out)
            texte = json.dumps(document, ensure_ascii=False)
            for mot in (
                "reprendre", "siren", "tva", "probabilite", "freeflow", "ancienne-piste",
                "9999", "Veille", "Maintenance", "Sous-page", "Alex", "Hors coffre", "cadence",
            ):
                self.assertNotIn(mot, texte, mot)
            dossier_valvert = document["dossiers"][0]
            self.assertEqual(
                set(dossier_valvert),
                {
                    "origine", "origine_fiche", "qui", "quoi", "adresse", "courriel",
                    "telephone", "travaux", "contact", "prochaine_action", "source",
                    "lignes", "serie", "brouillon",
                },
            )
            self.assertEqual(set(dossier_valvert["contact"]), {"prenom", "nom", "genre", "courriel"})
            self.assertNotIn("reprendre", document["series"][0])
            suivi = json.loads(notion.chemin_suivi(sortie).read_text(encoding="utf-8"))
            self.assertEqual(suivi["jour"], JOUR)
            self.assertTrue(suivi["dossiers"][0]["date_vide"])
            self.assertEqual(suivi["dossiers"][0]["prochaine_action"], JOUR)
            self.assertEqual(
                [ligne["raison"] for ligne in suivi["incomplets"]],
                ["statut clos", "sans travaux"],
            )
            self.assertEqual(suivi["types"]["Griffe"], "select")
            self.assertEqual(suivi["types_fiche"], {"freeflow_id": "rich_text"})
            self.assertEqual(
                out,
                "Mairie de Valvert — Refonte du site — 2 lignes — 2026-10-28\n"
                "incomplet — Atelier Clos — statut clos\n"
                "incomplet — Atelier Vide — sans travaux\n",
            )
            methodes = [(methode, chemin) for methode, chemin, _corps in transport.journal]
            posts = [chemin for methode, chemin in methodes if methode == "POST"]
            self.assertEqual(
                [chemin.split("/")[3] for chemin in posts],
                [notion.OPPORTUNITES, notion.ORGANISATIONS, notion.CONTACTS, notion.LIGNES, notion.SERIES],
            )
            self.assertEqual(
                transport.journal[0][2]["filter"],
                {"property": "Griffe", "select": {"equals": "À importer"}},
            )
            self.assertEqual(transport.journal[0][2]["page_size"], 100)
            for _methode, _chemin, corps in transport.journal[1:5]:
                self.assertNotIn("filter", corps)
            gets = [chemin for methode, chemin in methodes if methode == "GET"]
            self.assertEqual(
                [chemin.split("/")[3] for chemin in gets],
                [
                    dossier_valvert["origine"],
                    suivi["incomplets"][1]["origine"],
                    "66666666-6666-4666-8666-666666666666",
                ],
            )
            self.assertNotIn(suivi["incomplets"][0]["origine"], " ".join(gets))
            self.assertFalse(any(methode in {"PATCH", "DELETE"} for methode, _chemin in methodes))
            self.assertNotIn("properties", " ".join(gets))
            self.assertNotIn(JETON, json.dumps(transport.journal))

    def test_file_vide_ecrase_le_fichier_precedent(self):
        with tempfile.TemporaryDirectory() as dossier:
            dossier = Path(dossier)
            jeton = ecrire_jeton(dossier)
            sortie = dossier / "echange.json"
            sortie.write_text('{"version": 1, "dossiers": [{"qui": "ancien"}]}\n', encoding="utf-8")
            transport = Memoire()
            transport.sources[notion.OPPORTUNITES] = []
            code, out, err = lancer(
                ["lire", "--jeton", str(jeton), "--sortie", str(sortie), "--aujourdhui", JOUR],
                transport,
            )
            self.assertEqual(code, 0, err)
            self.assertEqual(out, "rien à importer\n")
            self.assertEqual(json.loads(sortie.read_text(encoding="utf-8")), {"version": 1, "dossiers": []})
            suivi = json.loads(notion.chemin_suivi(sortie).read_text(encoding="utf-8"))
            self.assertEqual(suivi["dossiers"], [])
            self.assertEqual(suivi["incomplets"], [])
            self.assertEqual(len(transport.journal), 1)
            self.assertEqual(transport.journal[0][0], "POST")

    def test_jeton_mauvais_mode_necrit_rien(self):
        with tempfile.TemporaryDirectory() as dossier:
            dossier = Path(dossier)
            jeton = ecrire_jeton(dossier, 0o644)
            sortie = dossier / "echange.json"
            transport = Memoire()
            code, out, err = lancer(["lire", "--jeton", str(jeton), "--sortie", str(sortie)], transport)
            self.assertEqual(code, 1)
            self.assertEqual(out, "")
            self.assertNotIn(JETON, err)
            self.assertIn("0600", err)
            self.assertFalse(sortie.exists())
            self.assertEqual(list(dossier.iterdir()), [jeton])
            self.assertEqual(transport.journal, [])

    def test_jeton_symlink_et_argument_sont_refuses(self):
        with tempfile.TemporaryDirectory() as dossier:
            dossier = Path(dossier)
            cible = ecrire_jeton(dossier)
            lien = dossier / "lien"
            lien.symlink_to(cible)
            code, _out, err = lancer(["lire", "--jeton", str(lien), "--sortie", str(dossier / "echange.json")], Memoire())
            self.assertEqual(code, 1)
            self.assertNotIn(JETON, err)
            code, _out, err = lancer(["lire", "--jeton", "ntn_live_exemple"], Memoire())
            self.assertEqual(code, 1)
            self.assertNotIn("ntn_live_exemple", err)
            code, _out, err = lancer(["lire", "--jeton", "secret_abc"], Memoire())
            self.assertEqual(code, 1)
            self.assertNotIn("secret_abc", err)
            blanc = ecrire_jeton(dossier, texte="jeton secret")
            code, _out, err = lancer(["lire", "--jeton", str(blanc)], Memoire())
            self.assertEqual(code, 1)
            self.assertNotIn("jeton secret", err)

    def test_date_illisible_ne_questionne_pas_notion(self):
        with tempfile.TemporaryDirectory() as dossier:
            dossier = Path(dossier)
            jeton = ecrire_jeton(dossier)
            transport = Memoire()
            code, _out, err = lancer(
                ["lire", "--jeton", str(jeton), "--aujourdhui", "28-10-2026"],
                transport,
            )
            self.assertEqual(code, 1)
            self.assertIn("date illisible", err)
            self.assertEqual(transport.journal, [])

    def test_montants(self):
        self.assertEqual(notion.montant_ligne(1200), "1200,00")
        self.assertEqual(notion.montant_ligne(400.5), "400,50")
        self.assertEqual(notion.montant_ligne(10.000009), "10,00")
        self.assertEqual(notion.montant_ligne(12.345), "12,345")
        self.assertEqual(notion.montant_ligne(0), "0,00")
        self.assertEqual(notion.montant_ligne("12,345"), "12,345")
        self.assertEqual(notion.montant_ligne("1 200,00"), "1200,00")
        self.assertIsNone(notion.centimes_proches(12.345))

    def test_jetons_le_plus_long_dabord(self):
        self.assertEqual(
            [len(source) for source, _cible in notion.VERS_COFFRE],
            sorted((len(source) for source, _cible in notion.VERS_COFFRE), reverse=True),
        )
        self.assertEqual(
            [len(source) for source, _cible in notion.VERS_NOTION],
            sorted((len(source) for source, _cible in notion.VERS_NOTION), reverse=True),
        )
        for balise, jeton in notion.VERS_COFFRE:
            self.assertEqual(notion.jetons_coffre(balise), jeton)
            self.assertEqual(notion.jetons_notion(jeton), balise)
        slash = "<Monsieur le Maire/Madame la Maire>"
        self.assertEqual(notion.jetons_coffre(slash), slash)
        self.assertEqual(notion.jetons_notion(slash), slash)
        self.assertEqual(
            notion.jetons_coffre("<prénom> <nom> <constat> <signature> " + slash),
            "{{prenom}} {{family_name}} {{constat}} {{signature}} " + slash,
        )

    def test_assemblage_anonyme(self):
        opp_a = page("a", {
            "Name": titre("Vitrine"),
            "Quoi": riche(""),
            "Statut": {"type": "status", "status": {"name": "À qualifier"}},
            "Série": {"type": "select", "select": {"name": "Déjà là"}},
            "Date prochaine action": {"type": "date", "date": None},
            "Organisation": {"type": "relation", "relation": [{"id": "o1"}]},
            "Contact": {"type": "relation", "relation": [{"id": "c1"}]},
            "Lignes": {"type": "relation", "relation": [{"id": "l0"}]},
            "Rue": riche("1 rue"),
        })
        opp_b = page("b", {
            "Name": titre("Close"),
            "Statut": {"type": "status", "status": {"name": "Ignorée"}},
            "Organisation": {"type": "relation", "relation": [{"id": "o2"}]},
        })
        opp_c = page("c", {
            "Name": titre("Corbeille"),
            "Statut": {"type": "status", "status": {"name": "À qualifier"}},
            "Organisation": {"type": "relation", "relation": [{"id": "o5"}]},
        })
        opp_d = page("d", {
            "Name": titre("Un"),
            "Quoi": riche("Un"),
            "Série": {"type": "select", "select": {"name": "Partagée"}},
            "Date prochaine action": {"type": "date", "date": {"start": "2026-11-02"}},
            "Organisation": {"type": "relation", "relation": [{"id": "o3"}]},
        })
        opp_e = page("e", {
            "Name": titre("Deux"),
            "Quoi": riche("Deux"),
            "Série": {"type": "select", "select": {"name": "Partagée"}},
            "Organisation": {"type": "relation", "relation": [{"id": "o4"}]},
            "Date prochaine action": {"type": "date", "date": None},
        })
        organisations = [
            page("o1", {"Qui": riche("Atelier du titre"), "Rue": riche("1 rue")}),
            page("o2", {"Qui": riche("Atelier clos")}),
            page("o3", {"Qui": riche("Atelier un")}),
            page("o4", {"Qui": riche("Atelier deux")}),
            page("o5", {"Qui": riche("Atelier jeté")}, in_trash=True),
        ]
        contacts = [page("c1", {"Prénom": riche("Alex"), "Nom": riche("")})]
        lignes = [page("l0", {"Mission": riche(""), "Montant HT": {"type": "number", "number": 0}})]
        series = [page("s1", {"Name": titre("Partagée")})]
        blocs = {
            "a": [
                titre2("Premier message"),
                paragraphe("Bonjour <prénom>."),
                titre2("Travaux"),
                paragraphe("Le <constat> reste."),
            ],
            "d": [
                titre2("Travaux"),
                {"type": "child_page", "child_page": {"title": "Sous-page"}},
            ],
            "e": [titre2("Travaux")],
            "s1": [
                titre2("hello"),
                {"type": "heading_3", "heading_3": {"rich_text": [{"plain_text": "Sujet"}]}},
                paragraphe("<prénom>"),
                {"type": "heading_3", "heading_3": {"rich_text": [{"plain_text": "Corps"}]}},
                paragraphe("<signature>"),
            ],
        }
        document, suivi = notion.assembler(
            [opp_a, opp_b, opp_c, opp_d, opp_e],
            organisations, contacts, lignes, series, blocs, JOUR,
        )
        self.assertEqual([ligne["origine"] for ligne in document["dossiers"]], ["a", "d", "e"])
        premier = document["dossiers"][0]
        self.assertEqual(premier["quoi"], "Vitrine")
        self.assertEqual(suivi["quoi_repris"], ["a"])
        self.assertNotIn("adresse", premier)
        self.assertNotIn("contact", premier)
        self.assertEqual(premier["travaux"], "Le <constat> reste.")
        self.assertNotIn("{{constat}}", premier["travaux"])
        self.assertEqual(premier["brouillon"], "Bonjour {{prenom}}.")
        self.assertEqual(premier["serie"], "Déjà là")
        self.assertEqual(premier["lignes"], [{"libelle": "", "montant": "0,00"}])
        self.assertEqual(premier["prochaine_action"], JOUR)
        self.assertTrue(suivi["dossiers"][0]["date_vide"])
        self.assertNotIn("travaux", document["dossiers"][1])
        self.assertNotIn("Sous-page", json.dumps(document))
        self.assertEqual(document["dossiers"][1]["prochaine_action"], "2026-11-02")
        self.assertFalse(suivi["dossiers"][1]["date_vide"])
        self.assertEqual([serie["nom"] for serie in document["series"]], ["Partagée"])
        self.assertEqual(document["series"][0]["moments"][0]["sujet"], "{{prenom}}")
        self.assertEqual(document["series"][0]["moments"][0]["corps"], "{{signature}}")
        self.assertNotIn("reprendre", document["series"][0])
        self.assertNotIn("reprendre", json.dumps(document))
        self.assertEqual(
            [(ligne["origine"], ligne["raison"]) for ligne in suivi["incomplets"]],
            [("b", "statut clos"), ("c", "sans organisation")],
        )
        tampon = io.StringIO()
        with contextlib.redirect_stdout(tampon):
            notion.afficher_lecture(document, suivi)
        self.assertIn("Quoi repris du titre", tampon.getvalue())
        self.assertEqual(notion.STATUTS_CLOS, frozenset({"Ignorée", "Gagnée", "Perdue"}))

    def test_relation_tronquee_est_relue(self):
        premiere = "44444444-4444-4444-8444-444444444441"
        seconde = "44444444-4444-4444-8444-444444444442"
        opp = "12121212-1212-4121-8121-121212121212"
        org = "13131313-1313-4131-8131-131313131313"
        transport = Memoire()
        transport.sources = {
            notion.OPPORTUNITES: [page(opp, {
                "Name": titre("Tronquée"),
                "Quoi": riche("Deux lignes"),
                "Statut": {"type": "status", "status": {"name": "À qualifier"}},
                "Date prochaine action": {"type": "date", "date": {"start": JOUR}},
                "Organisation": {"type": "relation", "relation": [{"id": org}]},
                "Lignes": {
                    "id": "Lx",
                    "type": "relation",
                    "has_more": True,
                    "relation": [{"id": premiere}],
                },
            })],
            notion.ORGANISATIONS: [page(org, {"Qui": riche("Atelier tronqué")})],
            notion.CONTACTS: [],
            notion.LIGNES: [
                page(premiere, {"Mission": riche("Une"), "Montant HT": {"type": "number", "number": 10}}),
                page(seconde, {"Mission": riche("Deux"), "Montant HT": {"type": "number", "number": 20}}),
            ],
            notion.SERIES: [],
        }
        transport.enfants = {opp: [titre2("Travaux"), paragraphe("Fait.")]}
        transport.proprietes[(opp, "Lx")] = [premiere, seconde]
        with tempfile.TemporaryDirectory() as dossier:
            dossier = Path(dossier)
            jeton = ecrire_jeton(dossier)
            sortie = dossier / "echange.json"
            code, _out, err = lancer(
                ["lire", "--jeton", str(jeton), "--sortie", str(sortie), "--aujourdhui", JOUR],
                transport,
            )
            self.assertEqual(code, 0, err)
            document = json.loads(sortie.read_text(encoding="utf-8"))
            self.assertEqual(
                [ligne["libelle"] for ligne in document["dossiers"][0]["lignes"]],
                ["Une", "Deux"],
            )
        chemins = [chemin for methode, chemin, _corps in transport.journal]
        self.assertIn(f"/v1/pages/{opp}/properties/Lx?page_size=100", chemins)
        self.assertLess(
            chemins.index(f"/v1/pages/{opp}/properties/Lx?page_size=100"),
            chemins.index(f"/v1/blocks/{opp}/children?page_size=100"),
        )

    def test_pagination_de_la_file(self):
        class Curseur:
            def __init__(self):
                self.corps = []

            def appel(self, _methode, _chemin, corps):
                self.corps.append(corps)
                if corps.get("start_cursor") == "suite":
                    return {"results": [{"object": "page", "id": "b"}], "has_more": False}
                return {"results": [{"object": "page", "id": "a"}], "has_more": True, "next_cursor": "suite"}

        transport = Curseur()
        pages = notion.interroger(
            transport,
            notion.OPPORTUNITES,
            {"filter": {"property": "Griffe", "select": {"equals": "À importer"}}},
        )
        self.assertEqual([ligne["id"] for ligne in pages], ["a", "b"])
        self.assertNotIn("start_cursor", transport.corps[0])
        self.assertEqual(transport.corps[1]["start_cursor"], "suite")
        self.assertEqual(transport.corps[0]["page_size"], 100)
        self.assertEqual(transport.corps[0]["filter"]["select"]["equals"], "À importer")

    def test_reporter_pose_les_identifiants_apres_application(self):
        with tempfile.TemporaryDirectory() as dossier:
            dossier = Path(dossier)
            jeton = ecrire_jeton(dossier)
            sortie = dossier / "echange.json"
            lecture = Memoire()
            lecture.charger_valvert()
            code, _out, err = lancer(
                ["lire", "--jeton", str(jeton), "--sortie", str(sortie), "--aujourdhui", JOUR],
                lecture,
            )
            self.assertEqual(code, 0, err)
            suivi = json.loads(notion.chemin_suivi(sortie).read_text(encoding="utf-8"))
            origine = suivi["dossiers"][0]["origine"]
            origine_fiche = suivi["dossiers"][0]["origine_fiche"]
            rapport = dossier / "rapport.json"
            rapport.write_text(json.dumps({
                "status": "applied",
                "result": {"dossiers": [{
                    "origine": origine,
                    "qui": "Mairie de Valvert",
                    "statut": "cree",
                    "fiche": FICHE,
                    "piste": PISTE,
                }]},
            }), encoding="utf-8")
            transport = Memoire()
            code, out, err = lancer(
                ["reporter", "--jeton", str(jeton), "--compte-rendu", str(rapport), "--suivi", str(notion.chemin_suivi(sortie))],
                transport,
            )
            self.assertEqual(code, 0, err)
            self.assertNotIn(JETON, out + err)
            self.assertIn(f"Importé — Mairie de Valvert — {PISTE}", out)
            patches = [(chemin, corps) for methode, chemin, corps in transport.journal if methode == "PATCH"]
            self.assertEqual(
                [chemin for chemin, _corps in patches],
                [
                    f"/v1/pages/{origine}",
                    f"/v1/pages/{origine_fiche}",
                    f"/v1/pages/{suivi['incomplets'][0]['origine']}",
                    f"/v1/pages/{suivi['incomplets'][1]['origine']}",
                ],
            )
            opp, fiche, clos, vide = (corps["properties"] for _chemin, corps in patches)
            self.assertEqual(set(opp), {"freeflow_id", "Griffe", "Date prochaine action"})
            self.assertEqual(opp["Griffe"], {"select": {"name": "Importé"}})
            self.assertEqual(opp["freeflow_id"]["rich_text"][0]["text"]["content"], PISTE)
            self.assertEqual(opp["Date prochaine action"], {"date": {"start": JOUR}})
            self.assertNotIn("is_datetime", json.dumps(opp))
            self.assertEqual(set(fiche), {"freeflow_id"})
            self.assertEqual(fiche["freeflow_id"]["rich_text"][0]["text"]["content"], FICHE)
            self.assertNotIn("Griffe", fiche)
            self.assertEqual(clos, {"Griffe": {"select": {"name": "Incomplet"}}})
            self.assertEqual(vide, {"Griffe": {"select": {"name": "Incomplet"}}})
            self.assertFalse(any(methode == "GET" for methode, _chemin, _corps in transport.journal))

    def test_date_deja_posee_et_dossier_incomplet(self):
        origine = "12121212-1212-4121-8121-121212121212"
        origine_fiche = "13131313-1313-4131-8131-131313131313"
        suivi = {
            "types": {"freeflow_id": "rich_text", "Griffe": "select"},
            "types_fiche": {"freeflow_id": "rich_text"},
            "dossiers": [{
                "origine": origine,
                "origine_fiche": origine_fiche,
                "qui": "Atelier",
                "date_vide": False,
                "prochaine_action": "2026-11-02",
            }],
            "incomplets": [],
        }
        transport = Memoire()
        with contextlib.redirect_stdout(io.StringIO()):
            notion.reporter(transport, {"dossiers": [{
                "origine": origine, "qui": "Atelier", "statut": "revise", "fiche": FICHE, "piste": PISTE,
            }]}, suivi)
        proprietes = transport.journal[0][2]["properties"]
        self.assertNotIn("Date prochaine action", proprietes)
        self.assertEqual(set(transport.journal[1][2]["properties"]), {"freeflow_id"})
        transport = Memoire()
        with contextlib.redirect_stdout(io.StringIO()):
            notion.reporter(transport, {"dossiers": [{
                "origine": origine, "qui": "Atelier", "statut": "incomplet",
            }]}, suivi)
        self.assertEqual(len(transport.journal), 1)
        self.assertEqual(
            transport.journal[0][2]["properties"],
            {"Griffe": {"select": {"name": "Incomplet"}}},
        )
        self.assertNotIn("freeflow_id", json.dumps(transport.journal))

    def test_report_rate_affiche_les_uuid_et_sarrete(self):
        with tempfile.TemporaryDirectory() as dossier:
            dossier = Path(dossier)
            jeton = ecrire_jeton(dossier)
            sortie = dossier / "echange.json"
            lecture = Memoire()
            lecture.charger_valvert()
            code, _out, err = lancer(
                ["lire", "--jeton", str(jeton), "--sortie", str(sortie), "--aujourdhui", JOUR],
                lecture,
            )
            self.assertEqual(code, 0, err)
            suivi_chemin = notion.chemin_suivi(sortie)
            suivi = json.loads(suivi_chemin.read_text(encoding="utf-8"))
            origine = suivi["dossiers"][0]["origine"]
            rapport = dossier / "rapport.json"
            rapport.write_text(json.dumps({
                "status": "applied",
                "result": {"dossiers": [{
                    "origine": origine,
                    "qui": "Mairie de Valvert",
                    "statut": "cree",
                    "fiche": FICHE,
                    "piste": PISTE,
                }]},
            }), encoding="utf-8")
            transport = Memoire()
            transport.echec_pages[origine] = JETON + " refusé"
            code, out, err = lancer(
                ["reporter", "--jeton", str(jeton), "--compte-rendu", str(rapport), "--suivi", str(suivi_chemin)],
                transport,
            )
            self.assertEqual(code, 1)
            self.assertNotIn("Importé", out)
            self.assertNotIn(JETON, err)
            self.assertIn("Le report n'a pas abouti.", err)
            self.assertIn(f"piste {PISTE}", err)
            self.assertIn(f"fiche {FICHE}", err)
            self.assertIn("… refusé", err)
            patches = [chemin for methode, chemin, _corps in transport.journal if methode == "PATCH"]
            self.assertEqual(patches, [f"/v1/pages/{origine}"])
            self.assertNotIn(suivi["dossiers"][0]["origine_fiche"], " ".join(patches))

    def test_compte_rendu_non_applique_necrit_rien(self):
        with tempfile.TemporaryDirectory() as dossier:
            dossier = Path(dossier)
            jeton = ecrire_jeton(dossier)
            rapport = dossier / "rapport.json"
            rapport.write_text('{"status": "pending_confirmation"}\n', encoding="utf-8")
            transport = Memoire()
            code, _out, err = lancer(
                ["reporter", "--jeton", str(jeton), "--compte-rendu", str(rapport), "--suivi", str(dossier / "absent.json")],
                transport,
            )
            self.assertEqual(code, 1)
            self.assertIn("pending_confirmation", err)
            self.assertNotIn(JETON, err)
            self.assertEqual(transport.journal, [])

    def test_phrases_ne_reecrit_que_si_lempreinte_change(self):
        catalogue = json.loads((FIXTURES / "valvert-series.json").read_text(encoding="utf-8"))
        rendus = notion.rendre_phrases(catalogue)
        self.assertEqual(
            notion.empreinte_blocs(rendus),
            (
                ("heading_2", "Défaut"),
                ("heading_3", "hello"),
                ("paragraph", "Sujet"),
                ("code", "Bonjour <prénom>"),
                ("paragraph", "Corps"),
                ("code", "<constat>\n\n<signature>"),
                ("heading_2", "Mairies"),
                ("heading_3", "hello"),
                ("paragraph", "Sujet"),
                ("code", "Bonjour <prénom> <nom>"),
                ("paragraph", "Corps"),
                ("code", "Un mot.\n\n<constat>\n\n<Monsieur le Maire/Madame la Maire>\n\n<signature>"),
            ),
        )
        preambule = [
            {"object": "block", "id": "pre-1", "type": "paragraph", "paragraph": {"rich_text": [{"plain_text": "Le miroir"}]}},
            {"object": "block", "id": "pre-2", "type": "paragraph", "paragraph": {"rich_text": [{"plain_text": "Les jetons"}]}},
            {"object": "block", "id": "pre-3", "type": "paragraph", "paragraph": {"rich_text": [{"plain_text": "Le constat"}]}},
        ]
        with tempfile.TemporaryDirectory() as dossier:
            dossier = Path(dossier)
            jeton = ecrire_jeton(dossier)
            series = dossier / "phrases.json"
            series.write_text(json.dumps(catalogue), encoding="utf-8")
            transport = Memoire()
            transport.enfants[notion.PHRASES] = preambule + blocs_notion(rendus, "deja")
            code, out, err = lancer(["phrases", "--jeton", str(jeton), "--series", str(series)], transport)
            self.assertEqual(code, 0, err)
            self.assertEqual(out, "phrases inchangées\n")
            self.assertEqual([methode for methode, _chemin, _corps in transport.journal], ["GET"])
            self.assertEqual([bloc["id"] for bloc in transport.enfants[notion.PHRASES][:3]], ["pre-1", "pre-2", "pre-3"])

            transport = Memoire()
            transport.enfants[notion.PHRASES] = preambule + [
                {"object": "block", "id": "ancien", "type": "paragraph", "paragraph": {"rich_text": [{"plain_text": "périmé"}]}},
            ]
            code, out, err = lancer(["phrases", "--jeton", str(jeton), "--series", str(series)], transport)
            self.assertEqual(code, 0, err)
            self.assertEqual(out, "phrases réécrites\n")
            suppressions = [chemin for methode, chemin, _corps in transport.journal if methode == "DELETE"]
            self.assertEqual(suppressions, ["/v1/blocks/ancien"])
            ajouts = [corps for methode, chemin, corps in transport.journal if methode == "PATCH"]
            self.assertEqual(len(ajouts), 1)
            self.assertEqual(notion.empreinte_blocs(ajouts[0]["children"]), notion.empreinte_blocs(rendus))
            self.assertNotIn(JETON, json.dumps(ajouts))
            self.assertEqual([bloc["id"] for bloc in transport.enfants[notion.PHRASES][:3]], ["pre-1", "pre-2", "pre-3"])
            deja = len(transport.journal)
            code, out, err = lancer(["phrases", "--jeton", str(jeton), "--series", str(series)], transport)
            self.assertEqual(code, 0, err)
            self.assertEqual(out, "phrases inchangées\n")
            self.assertEqual(
                [methode for methode, _chemin, _corps in transport.journal[deja:]],
                ["GET"],
            )

            catalogue_faux = dict(catalogue, version=2)
            series.write_text(json.dumps(catalogue_faux), encoding="utf-8")
            transport = Memoire()
            code, _out, err = lancer(["phrases", "--jeton", str(jeton), "--series", str(series)], transport)
            self.assertEqual(code, 1)
            self.assertIn("version", err)
            self.assertEqual(transport.journal, [])

    def test_phrases_par_paquets_de_cent(self):
        moments = [{"cle": f"m{indice}", "sujet": "Bonjour", "corps": "Suite"} for indice in range(20)]
        catalogue = {"version": 1, "series": [{"nom": "Lot", "moments": moments}]}
        self.assertEqual(len(notion.rendre_phrases(catalogue)), 101)
        with tempfile.TemporaryDirectory() as dossier:
            dossier = Path(dossier)
            jeton = ecrire_jeton(dossier)
            series = dossier / "phrases.json"
            series.write_text(json.dumps(catalogue), encoding="utf-8")
            transport = Memoire()
            transport.enfants[notion.PHRASES] = [
                {"id": f"pre-{indice}", "type": "paragraph", "paragraph": {"rich_text": [{"plain_text": "fixe"}]}}
                for indice in range(3)
            ]
            code, out, err = lancer(["phrases", "--jeton", str(jeton), "--series", str(series)], transport)
            self.assertEqual(code, 0, err)
            self.assertEqual(out, "phrases réécrites\n")
            paquets = [corps["children"] for methode, _chemin, corps in transport.journal if methode == "PATCH"]
            self.assertEqual([len(paquet) for paquet in paquets], [100, 1])
            self.assertEqual([bloc["id"] for bloc in transport.enfants[notion.PHRASES][:3]], ["pre-0", "pre-1", "pre-2"])

    def test_rich_text_decoupe_a_deux_mille(self):
        morceaux = notion.rich_text("a" * 4500)
        self.assertEqual([len(morceau["text"]["content"]) for morceau in morceaux], [2000, 2000, 500])
        bloc = {"type": "paragraph", "paragraph": {"rich_text": morceaux}}
        self.assertEqual(notion.texte_bloc(bloc), "a" * 4500)

    def test_erreur_http_retire_le_jeton(self):
        vu = {}

        def urlopen(requete, timeout=60):
            vu["auth"] = requete.get_header("Authorization")
            vu["version"] = requete.get_header("Notion-version")
            vu["timeout"] = timeout
            corps = json.dumps({"message": "mauvais " + JETON}).encode()
            raise urllib.error.HTTPError(requete.full_url, 401, "unauthorized", None, io.BytesIO(corps))

        urllib.request.urlopen = urlopen
        with self.assertRaises(notion.NotionErreur) as capture:
            notion.ClientHttp(JETON).appel("POST", "/v1/data_sources/x/query", {"page_size": 1})
        self.assertNotIn(JETON, str(capture.exception))
        self.assertIn("…", str(capture.exception))
        self.assertIsNone(capture.exception.__cause__)
        self.assertTrue(capture.exception.__suppress_context__)
        self.assertEqual(vu["auth"], "Bearer " + JETON)
        self.assertEqual(vu["version"], notion.VERSION_NOTION)

    def test_source_nouvre_pas_le_coffre(self):
        source = Path(__file__).resolve().with_name("echange_notion.py").read_text(encoding="utf-8")
        for mot in ("subprocess", "sqlite", "vault.db", "passphrase", "FREEFLOW_PASSPHRASE", "1b2dfa86", "os.system", "Popen"):
            self.assertNotIn(mot, source, mot)
        self.assertEqual(notion.OPPORTUNITES, "93a1d787-40e5-4ea9-a6fe-4eb3e5132706")
        self.assertEqual(notion.ORGANISATIONS, "61995cfa-952f-4006-9248-b35a456779a3")
        self.assertEqual(notion.CONTACTS, "414faeaf-4028-4d9a-935d-4fea1052cfed")
        self.assertEqual(notion.LIGNES, "f6f78908-a88b-4642-a6e8-1c28fa7625f7")
        self.assertEqual(notion.SERIES, "a973606b-0097-401b-bdf1-94b417ea04fc")
        self.assertEqual(notion.PHRASES, "3fb47a5c-6016-8178-849a-dd7b42c8be4e")
        self.assertEqual(notion.SELECT_FILE, "À importer")
        self.assertEqual(notion.VERSION_NOTION, "2025-09-03")


def blocs_notion(rendus, prefixe):
    blocs = []
    for indice, bloc in enumerate(rendus):
        sorte = bloc["type"]
        morceaux = []
        for morceau in bloc[sorte].get("rich_text") or []:
            contenu = ""
            if morceau.get("plain_text"):
                contenu = morceau["plain_text"]
            elif isinstance(morceau.get("text"), dict):
                contenu = morceau["text"].get("content") or ""
            morceaux.append({"type": "text", "plain_text": contenu})
        nouveau = {"object": "block", "id": f"{prefixe}-{indice}", "type": sorte, sorte: {"rich_text": morceaux}}
        if sorte == "code":
            nouveau[sorte]["language"] = "plain text"
        blocs.append(nouveau)
    return blocs


if __name__ == "__main__":
    unittest.main()
