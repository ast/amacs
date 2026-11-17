const fs = require("fs");

const DEFAULTS = {
  verbose: false,
  retries: 3,
};

class Runner {
  constructor(options) {
    this.options = { ...DEFAULTS, ...options };
  }

  run(tasks) {
    return tasks
      .filter((task) => task.enabled)
      .map((task) => {
        return task.name.toUpperCase();
      });
  }
}

function main() {
  const runner = new Runner({ verbose: true });
  for (const name of runner.run([])) {
    console.log(`task: ${name}`);
  }
  if (/^\d+$/.test("42")) {
    fs.writeFileSync("out.txt", "done\n");
  }
}

module.exports = { main };
