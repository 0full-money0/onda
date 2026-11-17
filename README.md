<div align="center">

<img src="docs/logo.png" width="120" alt="Logo d'Onda">

# Onda

**La musique en grand.**

Le clip du titre en plein écran, les paroles synchronisées, ta bibliothèque et une lumière qui bat au rythme de la musique.

⭐ **Si Onda te plaît, laisse une étoile au projet (bouton « Star » en haut à droite).** C'est gratuit, ça prend une seconde et ça donne énormément de force au projet.

[**Télécharger Onda**](../../releases/latest) · Windows · Mac · Linux

</div>

![Onda en lecture](docs/onda-ambiance.png)

---

## Ce que fait Onda

- **Le clip en fond d'écran** : quand un titre joue, son clip officiel occupe tout l'écran, dans une définition adaptée à ton écran et à ta connexion.
- **Les paroles synchronisées**, en grand, ligne par ligne.
- **Ta bibliothèque** : connecte ton compte une fois, et Onda affiche tes ajouts récents, tes écoutes récentes, ta musique et tes playlists (Apple Music, Spotify et YouTube Music).
- **La recherche** : titres, artistes, albums, avec une fiche pour lire, lire ensuite ou ajouter aux favoris.
- **La file « À suivre »** à droite : lire juste après, retirer, mettre en favori.
- **Le mode veille** : seulement le clip et l'heure, rien d'autre.
- **Les mises à jour automatiques** : quand une nouvelle version sort, Onda te la propose et l'installe en un clic (Windows, Mac et AppImage Linux).
- **La lumière des bords** qui suit le rythme, avec des étincelles.
- **Les couleurs de la pochette** reprises dans toute l'interface.

![Choix du service](docs/onda-choix.png)

## Installer Onda

Va sur la [page des téléchargements](../../releases/latest) et prends le fichier de ton système.

### Windows

1. Télécharge `Onda_…_x64-setup.exe` et ouvre-le.
2. Windows peut afficher « Windows a protégé votre ordinateur » : c'est normal pour une application qui n'a pas encore de certificat payant. Clique sur **Informations complémentaires**, puis **Exécuter quand même**.
3. Suis l'installateur. Onda apparaît dans le menu Démarrer.

### Mac

1. Télécharge `Onda_…_universal.dmg` (fonctionne sur les Mac Apple et Intel) et ouvre-le.
2. Glisse **Onda** dans le dossier **Applications**.
3. Au premier lancement, macOS peut dire que l'app « provient d'un développeur non identifié » : fais un **clic droit sur Onda → Ouvrir**, puis confirme. Si macOS indique que l'app est « endommagée », ouvre le Terminal et tape :
   ```
   xattr -cr /Applications/Onda.app
   ```

### Linux

- **N'importe quelle distribution** : télécharge `Onda_…_amd64.AppImage`, rends-le exécutable (`chmod +x Onda_*.AppImage`) puis lance-le.
- **Debian / Ubuntu** : télécharge le `.deb` et installe-le avec `sudo apt install ./Onda_*.deb`.

## Premiers pas

1. Au lancement, choisis ton service de musique dans la barre.
2. Connecte-toi dans la petite fenêtre qui s'ouvre, puis clique **Terminé**. Onda ne voit jamais ton mot de passe : la connexion se fait sur la page officielle du service.
3. Clique sur un titre : il se lance, avec son clip et ses paroles.

Raccourcis : `Espace` lecture / pause · `←` `→` parcourir · `Échap` revenir ou masquer.

## Bon à savoir

- **D'où vient le son** : Onda joue le **clip officiel** de chaque titre, via YouTube. Le son et l'image sont donc synchronisés, mais **des publicités YouTube peuvent apparaître** sur certains titres, et la version jouée peut parfois différer de l'album. Ton compte Apple Music, Spotify ou YouTube Music sert à retrouver ta bibliothèque, pas à produire le son : leurs lecteurs protégés refusent de fonctionner dans une autre application.
- **Ta bibliothèque** : Apple Music est testée. Spotify et YouTube Music sont prises en charge mais encore peu testées : si ta bibliothèque n'apparaît pas, signale-le dans l'onglet **Issues**. Pour Deezer, SoundCloud et Amazon Music, utilise la recherche.
- **Linux (.deb)** : la version `.deb` ne se met pas à jour toute seule ; télécharge la nouvelle version à la main, ou utilise l'AppImage.
- **Paroles** : elles viennent de [LRCLIB](https://lrclib.net). Certains titres n'en ont pas.
- **Systèmes** : Onda est testée sur Windows. Les versions Mac et Linux sont fabriquées automatiquement à partir du même code, mais n'ont pas encore été testées en conditions réelles : n'hésite pas à signaler ce qui ne marche pas dans l'onglet **Issues**.

## Compiler Onda soi-même

Il faut [Rust](https://rustup.rs) et [Node.js](https://nodejs.org), plus les [prérequis Tauri](https://tauri.app/start/prerequisites/) de ton système.

```
npm install
npm run tauri build
```

Les installateurs sont créés dans `src-tauri/target/release/bundle/`.

## Licence

Onda est **gratuite et son code est visible**, mais ce n'est pas un logiciel libre : tu peux l'utiliser et lire son code, mais pas la revendre, la redistribuer modifiée ni réutiliser son code ailleurs sans accord. Tout est expliqué dans [LICENSE](LICENSE).

Onda n'est ni affiliée ni approuvée par Apple, Spotify, YouTube, Deezer, SoundCloud ou Amazon. Leurs noms et logos appartiennent à leurs propriétaires.

---

<div align="center">

Tu aimes Onda ? ⭐ **Une étoile sur GitHub**, c'est le meilleur moyen de soutenir le projet.

</div>
