use pumpkin_data::Block;
use pumpkin_gametest::{GameTestContext, GameTestResult};
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};

pub(super) async fn test_nautilus_dries_out_on_land(test: GameTestContext) -> GameTestResult<()> {
    test.set_dimensions(5, 4, 5).await?;
    test.build_floor(5, 5, &Block::STONE).await?;
    for x in 0..5 {
        for z in 0..5 {
            for y in 1..=3 {
                let block = match (x, y, z) {
                    (1, 1..=2, 1) => &Block::WATER,
                    (3, 1..=2, 3) => &Block::AIR,
                    _ => &Block::STONE,
                };
                test.set_block_default(&BlockPos::new(x, y, z), block)
                    .await?;
            }
        }
    }

    test.execute_command(
        &BlockPos::new(1, 1, 1),
        "summon minecraft:nautilus ~ ~ ~ {Air:0s}",
    )
    .await?;
    test.execute_command(
        &BlockPos::new(3, 1, 3),
        "summon minecraft:nautilus ~ ~ ~ {Air:0s}",
    )
    .await?;

    test.succeed_when(|test| async move {
        test.assert_entities_present("minecraft:nautilus", 1)
            .await?;
        test.assert_entity_not_present_at("minecraft:nautilus", &BlockPos::new(3, 1, 3))
            .await?;
        test.assert_entity_not_present_at("minecraft:nautilus", &BlockPos::new(3, 2, 3))
            .await
    })?;
    Ok(())
}

pub(super) async fn test_nautilus_charges_the_mob_that_hurt_it(
    test: GameTestContext,
) -> GameTestResult<()> {
    test.set_dimensions(5, 5, 5).await?;
    test.build_floor(5, 5, &Block::STONE).await?;
    for x in 0..5 {
        for z in 0..5 {
            for y in 1..=4 {
                let wall = x == 0 || x == 4 || z == 0 || z == 4 || y == 4;
                let block = if wall { &Block::GLASS } else { &Block::WATER };
                test.set_block_default(&BlockPos::new(x, y, z), block)
                    .await?;
            }
        }
    }

    test.spawn_entity("minecraft:nautilus", Vector3::new(1.5, 1.0, 1.5))
        .await?;
    test.spawn_entity("minecraft:cod", Vector3::new(3.5, 1.0, 3.5))
        .await?;

    test.start_sequence()
        .then_execute_after(5, |test| async move {
            test.execute_command(
                &BlockPos::new(2, 1, 2),
                "damage @e[type=minecraft:nautilus,distance=..4,limit=1] 1 minecraft:mob_attack by @e[type=minecraft:cod,distance=..4,limit=1]",
            )
            .await?;
            Ok(())
        })
        .then_wait_until(|test| async move {
            test.assert_entity_present("minecraft:nautilus").await?;
            test.assert_entity_not_present("minecraft:cod").await
        })
        .then_succeed();
    Ok(())
}
