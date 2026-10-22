/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/nbody/NBodySystem.java).
 * Changed for ns-next: names are written out, and the six statements of
 * advance that set a velocity, too long for one line each with those names,
 * are spread over two. Upstream's file carries no licence notice; its
 * comment follows as it is.
 */
/* The Computer Language Benchmarks Game
 * http://shootout.alioth.debian.org/
 *
 * Based on nbody.java and adapted basde on the SOM version.
 */
package nbody;

public class NBodySystem {
  private final Body[] bodies;

  public NBodySystem() {
    bodies = createBodies();
  }

  public Body[] createBodies() {
    Body[] bodies = new Body[] {Body.sun(),
                                Body.jupiter(),
                                Body.saturn(),
                                Body.uranus(),
                                Body.neptune()};

    double momentumX = 0.0;
    double momentumY = 0.0;
    double momentumZ = 0.0;

    for (Body body : bodies) {
      momentumX += body.getVelocityX() * body.getMass();
      momentumY += body.getVelocityY() * body.getMass();
      momentumZ += body.getVelocityZ() * body.getMass();
    }

    bodies[0].offsetMomentum(momentumX, momentumY, momentumZ);

    return bodies;
  }

  public void advance(final double timeStep) {

    for (int first = 0; first < bodies.length; ++first) {
      Body firstBody = bodies[first];

      for (int second = first + 1; second < bodies.length; ++second) {
        Body secondBody = bodies[second];
        double deltaX = firstBody.getPositionX() - secondBody.getPositionX();
        double deltaY = firstBody.getPositionY() - secondBody.getPositionY();
        double deltaZ = firstBody.getPositionZ() - secondBody.getPositionZ();

        double distanceSquared = deltaX * deltaX + deltaY * deltaY + deltaZ * deltaZ;
        double distance = Math.sqrt(distanceSquared);
        double magnitude = timeStep / (distanceSquared * distance);

        firstBody.setVelocityX(
            firstBody.getVelocityX() - (deltaX * secondBody.getMass() * magnitude));
        firstBody.setVelocityY(
            firstBody.getVelocityY() - (deltaY * secondBody.getMass() * magnitude));
        firstBody.setVelocityZ(
            firstBody.getVelocityZ() - (deltaZ * secondBody.getMass() * magnitude));

        secondBody.setVelocityX(
            secondBody.getVelocityX() + (deltaX * firstBody.getMass() * magnitude));
        secondBody.setVelocityY(
            secondBody.getVelocityY() + (deltaY * firstBody.getMass() * magnitude));
        secondBody.setVelocityZ(
            secondBody.getVelocityZ() + (deltaZ * firstBody.getMass() * magnitude));
      }
    }

    for (Body body : bodies) {
      body.setPositionX(body.getPositionX() + timeStep * body.getVelocityX());
      body.setPositionY(body.getPositionY() + timeStep * body.getVelocityY());
      body.setPositionZ(body.getPositionZ() + timeStep * body.getVelocityZ());
    }
  }

  public double energy() {
    double energy = 0.0;

    for (int first = 0; first < bodies.length; ++first) {
      Body firstBody = bodies[first];
      energy += 0.5 * firstBody.getMass()
          * (firstBody.getVelocityX() * firstBody.getVelocityX() +
             firstBody.getVelocityY() * firstBody.getVelocityY() +
             firstBody.getVelocityZ() * firstBody.getVelocityZ());

      for (int second = first + 1; second < bodies.length; ++second) {
        Body secondBody = bodies[second];
        double deltaX = firstBody.getPositionX() - secondBody.getPositionX();
        double deltaY = firstBody.getPositionY() - secondBody.getPositionY();
        double deltaZ = firstBody.getPositionZ() - secondBody.getPositionZ();

        double distance = Math.sqrt(deltaX * deltaX + deltaY * deltaY + deltaZ * deltaZ);
        energy -= (firstBody.getMass() * secondBody.getMass()) / distance;
      }
    }
    return energy;
  }
}
