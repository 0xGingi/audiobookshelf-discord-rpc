const DiscordRPC = require('discord-rpc');
const https = require('https');
const fs = require('fs');
const config = JSON.parse(fs.readFileSync('config.json', 'utf8'));

const clientId = config.discordClientId;
const audiobookshelfUrl = config.audiobookshelfUrl;
const audiobookshelfToken = config.audiobookshelfToken;
const userId = config.audiobookshelfUserId;
const rpc = new DiscordRPC.Client({ transport: 'ipc' });

async function getCoverPath(title, author) {
    return new Promise((resolve, reject) => {
        const encodedTitle = encodeURIComponent(title);
        const encodedAuthor = encodeURIComponent(author);
        const coverOptions = {
            hostname: new URL(audiobookshelfUrl).hostname,
            path: `/api/search/covers?title=${encodedTitle}&author=${encodedAuthor}&provider=audible`,
            method: 'GET',
            headers: {
                'Authorization': `Bearer ${audiobookshelfToken}`
            }
        };

        const req = https.request(coverOptions, (res) => {
            let data = '';

            res.on('data', (chunk) => {
                data += chunk;
            });

            res.on('end', () => {
                try {
                    const response = JSON.parse(data);
                    const coverUrl = response.results[0];
                    resolve(coverUrl);
                } catch (error) {
                    console.error('Error fetching cover URL:', error);
                    reject(error);
                }
            });
        });

        req.on('error', (error) => {
            console.error('Error making request:', error);
            reject(error);
        });

        req.end();
    });
}
let lastKnownTime = null;
let isPaused = false;

async function setActivity() {
  const options = {
    hostname: new URL(audiobookshelfUrl).hostname,
    path: `/api/me/listening-sessions?itemsPerPage=1`,
    method: 'GET',
    headers: {
      'Authorization': `Bearer ${audiobookshelfToken}`
    }
  };

  const req = https.request(options, (res) => {
    let data = '';

    res.on('data', (chunk) => {
      data += chunk;
    });

    res.on('end', () => {
      try {
        const session = JSON.parse(data).sessions[0];
        const bookName = session.displayTitle;
        const author = session.author;
        const currentTime = session.currentTime;
        const totalTime = formatTime(session.duration);

        if (lastKnownTime === null) {
          lastKnownTime = currentTime;
        } else if (currentTime === lastKnownTime) {
          if (!isPaused) {
            console.log('Book paused. Clearing activity.');
            rpc.clearActivity();
            isPaused = true;
          }
          return;
        } else {
          isPaused = false;
        }

        lastKnownTime = currentTime;

        if (!isPaused) {
          getCoverPath(bookName, author).then(coverUrl => {
            rpc.setActivity({
              details: `Listening to ${bookName}`,
              state: `${formatTime(currentTime)} / ${totalTime}`,
              largeImageKey: coverUrl,
              largeImageText: bookName,
              instance: false,
            });
          }).catch(error => {
            console.error('Error fetching cover URL:', error);
          });
        }
      } catch (error) {
        console.error('Error fetching listening session:', error);
      }
    });
  });

  req.on('error', (error) => {
    console.error('Error making request:', error);
  });

  req.end();
}

function formatTime(seconds) {
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const remainingSeconds = Math.floor(seconds % 60);
  return `${hours.toString().padStart(2, '0')}:${minutes.toString().padStart(2, '0')}:${remainingSeconds.toString().padStart(2, '0')}`;
}

rpc.on('ready', () => {
  console.log('Audiobookshelf Discord RPC Connected!');
  setActivity();
  setInterval(() => {
    setActivity();
  }, 15000);
});

rpc.login({ clientId }).catch(console.error);
